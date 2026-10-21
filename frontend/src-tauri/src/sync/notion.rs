//! Notion target: turn the same meeting Markdown into blocks and create a page.
//!
//! Two constraints drive the shape of this module:
//!
//! - Notion accepts at most 100 blocks per request, so a transcript is split
//!   into chunks and appended after the page is created.
//! - Notion's database model moved to data sources in API version
//!   `2025-09-03`; a page must be created under a `data_source_id`, which is
//!   discovered from the database the user configured.
//!
//! Markdown parsing lives here rather than in the frontend because the network
//! call and the block limit live here: keeping conversion and chunking together
//! means the 100-block rule cannot drift away from the code that sends blocks.

use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration;

/// Hard limit per create/append request, from Notion's request limits.
pub const MAX_BLOCKS_PER_REQUEST: usize = 100;

/// Notion caps one rich-text object at 2000 characters. Leave a little room so
/// a chunk never lands exactly on the limit.
const MAX_RICH_TEXT_CHARS: usize = 1900;

/// Pinned API version. Data-source parents (used below) require at least
/// `2025-09-03`; this later version only changes block positioning, trash
/// fields and the transcription block name, none of which this module uses.
pub const NOTION_VERSION: &str = "2026-03-11";

const NOTION_API_BASE: &str = "https://api.notion.com/v1";

// ===== Database id =====

/// Maximal runs of hexadecimal characters, in the order they appear.
fn hex_runs(input: &str) -> Vec<&str> {
    let mut runs = Vec::new();
    let mut start: Option<usize> = None;

    for (index, c) in input.char_indices() {
        if c.is_ascii_hexdigit() {
            if start.is_none() {
                start = Some(index);
            }
        } else if let Some(from) = start.take() {
            runs.push(&input[from..index]);
        }
    }

    if let Some(from) = start {
        runs.push(&input[from..]);
    }

    runs
}

/// Accept a plain id, a UUID with dashes, or a pasted Notion URL.
///
/// A database URL looks like `.../<workspace>/<title>-<database id>?v=<view id>`;
/// the query string is dropped first so the view id can never be mistaken for
/// the database id.
pub fn normalize_database_id(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Enter the Notion database ID in Settings → Sync first.".to_string());
    }

    let path = trimmed
        .split(['?', '#'])
        .next()
        .unwrap_or(trimmed)
        .trim_end_matches('/');

    if let Some(id) = hex_runs(path)
        .into_iter()
        .rev()
        .find(|run| run.len() == 32)
    {
        return Ok(id.to_ascii_lowercase());
    }

    let compact: String = path.chars().filter(|c| *c != '-').collect();
    if compact.len() == 32 && compact.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(compact.to_ascii_lowercase());
    }

    Err(
        "That does not look like a Notion database ID. Copy the 32-character ID from the \
         database URL (the part after the workspace name, before any `?v=`)."
            .to_string(),
    )
}

// ===== Markdown -> blocks =====

/// One run of text with the emphasis Notion can represent.
#[derive(Debug, Clone, PartialEq)]
pub struct InlineText {
    pub text: String,
    pub bold: bool,
    pub code: bool,
    pub link: Option<String>,
}

fn push_segment(
    out: &mut Vec<InlineText>,
    buffer: &mut String,
    bold: bool,
    code: bool,
    link: &Option<String>,
) {
    if buffer.is_empty() {
        return;
    }

    let text = std::mem::take(buffer);

    // Merge with the previous run when the formatting is identical. Notion caps
    // a block at 100 rich-text objects, so splitting `**a****b**` into two runs
    // would waste that budget and make the text render identically anyway.
    if let Some(last) = out.last_mut() {
        if last.bold == bold && last.code == code && last.link == *link {
            last.text.push_str(&text);
            return;
        }
    }

    out.push(InlineText {
        text,
        bold,
        code,
        link: link.clone(),
    });
}

/// Parse the inline subset this app's Markdown produces: bold, code and links.
///
/// Deliberately not a full Markdown parser: unmatched markers are left as
/// literal text, so a summary containing a stray `*` is never silently eaten.
pub fn parse_inline(input: &str) -> Vec<InlineText> {
    let chars: Vec<char> = input.chars().collect();
    let mut out = Vec::new();
    let mut buffer = String::new();
    let mut bold = false;
    let mut code = false;
    let mut link: Option<String> = None;
    let mut index = 0;

    while index < chars.len() {
        // Bold: `**` or `__`, only outside code spans.
        if !code && index + 1 < chars.len() {
            let is_star = chars[index] == '*' && chars[index + 1] == '*';
            let is_underscore = chars[index] == '_' && chars[index + 1] == '_';
            if is_star || is_underscore {
                push_segment(&mut out, &mut buffer, bold, code, &link);
                bold = !bold;
                index += 2;
                continue;
            }
        }

        if chars[index] == '`' {
            push_segment(&mut out, &mut buffer, bold, code, &link);
            code = !code;
            index += 1;
            continue;
        }

        // Link: `[label](url)`. Anything that does not parse is left literal.
        if !code && chars[index] == '[' {
            let mut closing = index + 1;
            while closing < chars.len() && chars[closing] != ']' {
                closing += 1;
            }

            if closing + 1 < chars.len() && chars[closing] == ']' && chars[closing + 1] == '(' {
                let mut end = closing + 2;
                while end < chars.len() && chars[end] != ')' {
                    end += 1;
                }

                if end < chars.len() {
                    let label: String = chars[index + 1..closing].iter().collect();
                    let url: String = chars[closing + 2..end].iter().collect();
                    let url = url.trim();

                    if !url.is_empty() {
                        push_segment(&mut out, &mut buffer, bold, code, &link);

                        let previous_link = link.take();
                        link = Some(url.to_string());
                        buffer.push_str(&label);
                        push_segment(&mut out, &mut buffer, bold, code, &link);
                        link = previous_link;

                        index = end + 1;
                        continue;
                    }
                }
            }
        }

        buffer.push(chars[index]);
        index += 1;
    }

    push_segment(&mut out, &mut buffer, bold, code, &link);
    out
}

fn annotations(bold: bool, code: bool) -> Value {
    json!({
        "bold": bold,
        "italic": false,
        "strikethrough": false,
        "underline": false,
        "code": code,
        "color": "default",
    })
}

fn rich_text(text: &str) -> Vec<Value> {
    parse_inline(text)
        .into_iter()
        .filter(|segment| !segment.text.is_empty())
        .map(|segment| {
            let mut text_object = json!({ "content": segment.text });
            if let Some(url) = segment.link {
                text_object["link"] = json!({ "url": url });
            }

            json!({
                "type": "text",
                "text": text_object,
                "annotations": annotations(segment.bold, segment.code),
            })
        })
        .collect()
}

/// Code blocks must keep their content verbatim, so no inline parsing happens.
fn plain_rich_text(text: &str) -> Vec<Value> {
    if text.is_empty() {
        return Vec::new();
    }

    vec![json!({
        "type": "text",
        "text": { "content": text },
        "annotations": annotations(false, false),
    })]
}

fn make_block(kind: &str, rich_text: Vec<Value>) -> Value {
    let mut payload = serde_json::Map::new();
    payload.insert("rich_text".into(), Value::Array(rich_text));

    if kind == "code" {
        payload.insert("language".into(), json!("plain text"));
    }

    let mut block = serde_json::Map::new();
    block.insert("object".into(), json!("block"));
    block.insert("type".into(), json!(kind));
    block.insert(kind.to_string(), Value::Object(payload));
    Value::Object(block)
}

fn todo_block(rich_text: Vec<Value>, checked: bool) -> Value {
    let mut block = make_block("to_do", rich_text);
    if let Some(payload) = block.get_mut("to_do").and_then(Value::as_object_mut) {
        payload.insert("checked".into(), Value::Bool(checked));
    }
    block
}

/// Split a line that is longer than Notion's rich-text limit.
fn split_text(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut rest = text;

    loop {
        if rest.chars().count() <= MAX_RICH_TEXT_CHARS {
            if !rest.is_empty() {
                chunks.push(rest.to_string());
            }
            return chunks;
        }

        let mut boundary = rest.len();
        for (count, (index, _)) in rest.char_indices().enumerate() {
            if count == MAX_RICH_TEXT_CHARS {
                boundary = index;
                break;
            }
        }

        // Prefer a word boundary in the second half, so a paragraph is not cut
        // mid-word when it does not have to be.
        let slice = &rest[..boundary];
        let cut = slice
            .rfind(|c: char| c.is_whitespace())
            .filter(|index| *index > boundary / 2)
            .unwrap_or(boundary);

        let chunk = rest[..cut].trim_end();
        if !chunk.is_empty() {
            chunks.push(chunk.to_string());
        }

        rest = rest[cut..].trim_start();
        if rest.is_empty() {
            return chunks;
        }
    }
}

fn push_text(blocks: &mut Vec<Value>, kind: &str, text: &str) {
    for chunk in split_text(text.trim()) {
        let rich = rich_text(&chunk);
        if !rich.is_empty() {
            blocks.push(make_block(kind, rich));
        }
    }
}

fn push_code(blocks: &mut Vec<Value>, text: &str) {
    if text.trim().is_empty() {
        return;
    }

    for chunk in split_text(text) {
        blocks.push(make_block("code", plain_rich_text(&chunk)));
    }
}

fn heading(trimmed: &str) -> Option<(&'static str, &str)> {
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }

    let text = trimmed[hashes..].strip_prefix(' ')?.trim();
    if text.is_empty() {
        return None;
    }

    let kind = match hashes {
        1 => "heading_1",
        2 => "heading_2",
        _ => "heading_3",
    };

    Some((kind, text))
}

fn todo(trimmed: &str) -> Option<(bool, &str)> {
    for (prefix, checked) in [("- [ ] ", false), ("- [x] ", true), ("- [X] ", true)] {
        if let Some(text) = trimmed.strip_prefix(prefix) {
            let text = text.trim();
            if !text.is_empty() {
                return Some((checked, text));
            }
        }
    }

    None
}

fn bullet(trimmed: &str) -> Option<&str> {
    for prefix in ["- ", "* ", "+ "] {
        if let Some(text) = trimmed.strip_prefix(prefix) {
            let text = text.trim();
            if !text.is_empty() {
                return Some(text);
            }
        }
    }

    None
}

fn numbered(trimmed: &str) -> Option<&str> {
    let digits = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }

    let rest = &trimmed[digits..];
    let text = rest
        .strip_prefix(". ")
        .or_else(|| rest.strip_prefix(") "))?
        .trim();

    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn quote(trimmed: &str) -> Option<&str> {
    let text = trimmed.strip_prefix('>')?.trim();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Convert the meeting Markdown into Notion block objects.
///
/// The Markdown is what `buildMeetingMarkdown` writes: headings, bullet lists,
/// bold transcript lines and paragraphs. Anything unrecognised becomes a
/// paragraph, so no text is ever dropped.
pub fn markdown_to_blocks(markdown: &str) -> Vec<Value> {
    let mut blocks = Vec::new();
    let mut code: Option<String> = None;

    for raw_line in markdown.lines() {
        let line = raw_line.trim_end_matches('\r');
        let trimmed = line.trim();

        if code.is_some() {
            if trimmed.starts_with("```") {
                let content = code.take().unwrap_or_default();
                push_code(&mut blocks, &content);
            } else {
                let buffer = code.as_mut().expect("checked above");
                if !buffer.is_empty() {
                    buffer.push('\n');
                }
                buffer.push_str(line);
            }
            continue;
        }

        if trimmed.starts_with("```") {
            code = Some(String::new());
            continue;
        }

        if trimmed.is_empty() {
            continue;
        }

        if let Some((kind, text)) = heading(trimmed) {
            push_text(&mut blocks, kind, text);
        } else if let Some((checked, text)) = todo(trimmed) {
            for chunk in split_text(text) {
                let rich = rich_text(&chunk);
                if !rich.is_empty() {
                    blocks.push(todo_block(rich, checked));
                }
            }
        } else if let Some(text) = bullet(trimmed) {
            push_text(&mut blocks, "bulleted_list_item", text);
        } else if let Some(text) = numbered(trimmed) {
            push_text(&mut blocks, "numbered_list_item", text);
        } else if let Some(text) = quote(trimmed) {
            push_text(&mut blocks, "quote", text);
        } else if matches!(trimmed, "---" | "***" | "___") {
            blocks.push(json!({ "object": "block", "type": "divider", "divider": {} }));
        } else {
            push_text(&mut blocks, "paragraph", trimmed);
        }
    }

    // An unterminated fence still carries text; treat the rest as code.
    if let Some(content) = code {
        push_code(&mut blocks, &content);
    }

    blocks
}

/// Group blocks into request-sized chunks.
pub fn chunk_blocks(blocks: Vec<Value>, max: usize) -> Vec<Vec<Value>> {
    if blocks.is_empty() {
        return Vec::new();
    }

    let max = max.max(1);
    let mut chunks = Vec::new();
    let mut current = Vec::with_capacity(max);

    for block in blocks {
        current.push(block);
        if current.len() == max {
            chunks.push(std::mem::replace(&mut current, Vec::with_capacity(max)));
        }
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    chunks
}

// ===== Notion API =====

#[derive(Debug, Clone)]
pub struct NotionTarget {
    pub database_id: String,
    pub database_title: Option<String>,
    pub data_source_id: String,
    pub data_source_name: Option<String>,
    pub title_property: String,
}

#[derive(Debug, Clone)]
pub struct CreatedPage {
    pub id: String,
    pub url: Option<String>,
}

#[derive(Deserialize)]
struct DataSourceRef {
    id: String,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
struct DatabaseResponse {
    #[serde(default)]
    data_sources: Vec<DataSourceRef>,
    #[serde(default)]
    title: Vec<RichTextItem>,
}

#[derive(Deserialize)]
struct DataSourceResponse {
    #[serde(default)]
    properties: HashMap<String, PropertySchema>,
    #[serde(default)]
    title: Vec<RichTextItem>,
}

#[derive(Deserialize)]
struct PropertySchema {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct RichTextItem {
    #[serde(default)]
    plain_text: String,
}

#[derive(Deserialize)]
struct PageResponse {
    id: String,
    #[serde(default)]
    url: Option<String>,
}

#[derive(Deserialize)]
struct NotionErrorBody {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

fn plain_text(items: &[RichTextItem]) -> Option<String> {
    let joined: String = items.iter().map(|item| item.plain_text.as_str()).collect();
    let joined = joined.trim();
    if joined.is_empty() {
        None
    } else {
        Some(joined.to_string())
    }
}

/// Request builder with Notion's auth and version headers, but no content type.
///
/// File-upload calls set their own Content-Type (JSON or multipart), so the JSON
/// default from request() must not be applied to them.
pub(crate) fn authorized_request(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: &str,
    token: &str,
) -> reqwest::RequestBuilder {
    client
        .request(method, url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Notion-Version", NOTION_VERSION)
}

fn request(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: &str,
    token: &str,
) -> reqwest::RequestBuilder {
    authorized_request(client, method, url, token).header("Content-Type", "application/json")
}

/// Map a failed response onto a message that says what to change.
fn notion_error_message(
    status: reqwest::StatusCode,
    body: &str,
    database_id: Option<&str>,
) -> String {
    let parsed = serde_json::from_str::<NotionErrorBody>(body).ok();
    let code = parsed
        .as_ref()
        .and_then(|error| error.code.as_deref())
        .unwrap_or("");
    let message = parsed
        .as_ref()
        .and_then(|error| error.message.as_deref())
        .unwrap_or("");

    let id = database_id.unwrap_or("the configured database");
    let code_suffix = if code.is_empty() {
        String::new()
    } else {
        format!(" {code}")
    };
    let detail = if message.is_empty() {
        body.chars().take(300).collect::<String>()
    } else {
        message.to_string()
    };

    match status.as_u16() {
        401 => "Notion rejected the integration token (401 unauthorized). Open Settings → Sync, \
                paste a current internal integration token, and save it again."
            .to_string(),
        403 => format!(
            "The integration does not have access to this database (403{code_suffix}). In Notion \
             open the database, choose ••• → Connections, and add your integration."
        ),
        404 => format!(
            "Notion could not find database {id} (404{code_suffix}). Check the database ID and \
             share the database with your integration: open it in Notion → ••• → Connections. \
             Notion said: {detail}"
        ),
        400 if code == "object_not_found" => format!(
            "Notion could not find database {id} (400 object_not_found). Check the ID and that the \
             database is shared with your integration. Notion said: {detail}"
        ),
        400 if code == "validation_error" => format!(
            "Notion rejected the page as invalid (400 validation_error). Check that the ID is a \
             database rather than a page, and that it has a title property. Notion said: {detail}"
        ),
        429 => "Notion is rate limiting requests (429). Wait a minute, then sync again.".to_string(),
        _ => format!("Notion returned an error {status}{code_suffix}: {detail}"),
    }
}

/// Send a request, retrying the small number of rate-limited responses.
///
/// Notion allows roughly three requests per second. A long transcript produces
/// several append calls, so a 429 is expected rather than exceptional; retrying
/// twice with `Retry-After` respects what Notion asked for.
pub(crate) async fn execute<F>(build: F, database_id: Option<&str>) -> Result<Value, String>
where
    F: Fn() -> reqwest::RequestBuilder,
{
    let mut attempt = 0u32;

    loop {
        let response = build().send().await.map_err(|error| {
            format!(
                "Could not reach Notion: {error}. Check this computer's network connection and \
                 try again."
            )
        })?;

        let status = response.status();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok());

        if status.is_success() {
            let text = response
                .text()
                .await
                .map_err(|error| format!("Could not read Notion's response: {error}"))?;

            if text.trim().is_empty() {
                return Ok(Value::Null);
            }

            return serde_json::from_str(&text)
                .map_err(|error| format!("Notion returned an unexpected response: {error}"));
        }

        let body = response.text().await.unwrap_or_default();

        if status.as_u16() == 429 && attempt < 2 {
            let wait = retry_after.unwrap_or(1 << attempt).clamp(1, 30);
            log::warn!("Notion rate limited the request; retrying in {wait}s");
            tokio::time::sleep(Duration::from_secs(wait)).await;
            attempt += 1;
            continue;
        }

        return Err(notion_error_message(status, &body, database_id));
    }
}

/// Resolve the database's data source and its title property.
///
/// The user supplies a database ID (what a Notion URL shows), but pages must be
/// created under a data source. The title property name is user-defined
/// ("Name", "Title", …), so it has to be read from the schema rather than
/// assumed.
pub async fn discover_target(
    client: &reqwest::Client,
    token: &str,
    database_id: &str,
) -> Result<NotionTarget, String> {
    let database_url = format!("{NOTION_API_BASE}/databases/{database_id}");
    let database_json = execute(
        || request(client, reqwest::Method::GET, &database_url, token),
        Some(database_id),
    )
    .await?;

    let database: DatabaseResponse = serde_json::from_value(database_json)
        .map_err(|error| format!("Could not read the Notion database response: {error}"))?;

    let source = database.data_sources.first().ok_or_else(|| {
        format!(
            "Notion returned no data sources for database {database_id}. Open the database in \
             Notion and confirm it still exists and is shared with your integration."
        )
    })?;

    let data_source_url = format!("{NOTION_API_BASE}/data_sources/{}", source.id);
    let source_json = execute(
        || request(client, reqwest::Method::GET, &data_source_url, token),
        Some(database_id),
    )
    .await?;

    let source_schema: DataSourceResponse = serde_json::from_value(source_json)
        .map_err(|error| format!("Could not read the Notion data source response: {error}"))?;

    let title_property = source_schema
        .properties
        .iter()
        .find(|(_, schema)| schema.kind == "title")
        .map(|(name, _)| name.clone())
        .ok_or_else(|| {
            format!(
                "Database {database_id} has no title property, so a page title cannot be set. \
                 Add a title column in Notion and try again."
            )
        })?;

    Ok(NotionTarget {
        database_id: database_id.to_string(),
        database_title: plain_text(&database.title),
        data_source_id: source.id.clone(),
        data_source_name: source
            .name
            .clone()
            .or_else(|| plain_text(&source_schema.title)),
        title_property,
    })
}

/// Create the page, optionally with the first chunk of content.
pub async fn create_page(
    client: &reqwest::Client,
    token: &str,
    target: &NotionTarget,
    title: &str,
    blocks: Vec<Value>,
) -> Result<CreatedPage, String> {
    let title: String = title.trim().chars().take(2000).collect();

    let mut properties = serde_json::Map::new();
    properties.insert(
        target.title_property.clone(),
        json!({
            "title": [{
                "type": "text",
                "text": { "content": title },
            }],
        }),
    );

    let mut body = serde_json::Map::new();
    body.insert(
        "parent".into(),
        json!({
            "type": "data_source_id",
            "data_source_id": target.data_source_id,
        }),
    );
    body.insert("properties".into(), Value::Object(properties));

    // Notion rejects an empty `children` array; omit it when there is no text.
    if !blocks.is_empty() {
        body.insert("children".into(), Value::Array(blocks));
    }

    let body = Value::Object(body);
    let url = format!("{NOTION_API_BASE}/pages");

    let response = execute(
        || request(client, reqwest::Method::POST, &url, token).json(&body),
        Some(&target.database_id),
    )
    .await?;

    let page: PageResponse = serde_json::from_value(response)
        .map_err(|error| format!("Could not read the created Notion page: {error}"))?;

    Ok(CreatedPage {
        id: page.id,
        url: page.url,
    })
}

/// Append one chunk (at most [`MAX_BLOCKS_PER_REQUEST`]) to an existing page.
pub async fn append_children(
    client: &reqwest::Client,
    token: &str,
    page_id: &str,
    blocks: Vec<Value>,
) -> Result<(), String> {
    if blocks.is_empty() {
        return Ok(());
    }

    let url = format!("{NOTION_API_BASE}/blocks/{page_id}/children");
    let body = json!({ "children": blocks });

    execute(
        || request(client, reqwest::Method::PATCH, &url, token).json(&body),
        None,
    )
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_kind(block: &Value) -> &str {
        block.get("type").and_then(Value::as_str).unwrap_or("")
    }

    #[test]
    fn database_ids_are_accepted_in_plain_and_uuid_form() {
        let plain = "0123456789abcdef0123456789abcdef";
        assert_eq!(normalize_database_id(plain).unwrap(), plain);

        let dashed = "01234567-89ab-cdef-0123-456789abcdef";
        assert_eq!(normalize_database_id(dashed).unwrap(), plain);
    }

    #[test]
    fn a_pasted_database_url_uses_the_path_id_not_the_view_id() {
        let url = "https://www.notion.so/acme/Q3-Plan-0123456789abcdef0123456789abcdef?v=ffffffffffffffffffffffffffffffff";
        assert_eq!(
            normalize_database_id(url).unwrap(),
            "0123456789abcdef0123456789abcdef"
        );
    }

    #[test]
    fn invalid_database_ids_explain_where_to_find_one() {
        let error = normalize_database_id("not-an-id").unwrap_err();
        assert!(error.contains("32-character"));

        assert!(normalize_database_id("").is_err());
        assert!(normalize_database_id("0123456789abcdef").is_err());
    }

    #[test]
    fn inline_bold_code_and_links_are_parsed() {
        let segments = parse_inline("**00:01 · Speaker** said `yes` in [the spec](https://example.com/spec)");
        assert_eq!(segments[0].text, "00:01 · Speaker");
        assert!(segments[0].bold);
        assert_eq!(segments[1].text, " said ");
        assert!(!segments[1].bold);
        assert_eq!(segments[2].text, "yes");
        assert!(segments[2].code);
        assert_eq!(
            segments.iter().find(|s| s.link.is_some()).unwrap().link.as_deref(),
            Some("https://example.com/spec")
        );
    }

    #[test]
    fn unmatched_markers_stay_literal() {
        let segments = parse_inline("2 * 3 and [not a link");
        let text: String = segments.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(text, "2 * 3 and [not a link");
    }

    #[test]
    fn markdown_lines_become_the_matching_notion_block_types() {
        let markdown = "\
# Weekly Sync

- **Date:** 3 Feb 2026
- [x] Send the notes
1. First
> A quote
---

## Summary

Paragraph text.
";
        let blocks = markdown_to_blocks(markdown);
        let kinds: Vec<&str> = blocks.iter().map(block_kind).collect();

        assert_eq!(
            kinds,
            vec![
                "heading_1",
                "bulleted_list_item",
                "to_do",
                "numbered_list_item",
                "quote",
                "divider",
                "heading_2",
                "paragraph",
            ]
        );

        let todo = blocks
            .iter()
            .find(|block| block_kind(block) == "to_do")
            .unwrap();
        assert_eq!(todo["to_do"]["checked"], json!(true));
    }

    #[test]
    fn fenced_code_is_kept_verbatim_as_a_code_block() {
        let markdown = "```\nlet x = **not bold**;\n```\n";
        let blocks = markdown_to_blocks(markdown);
        assert_eq!(blocks.len(), 1);
        assert_eq!(block_kind(&blocks[0]), "code");
        assert_eq!(
            blocks[0]["code"]["rich_text"][0]["text"]["content"],
            json!("let x = **not bold**;")
        );
        assert_eq!(blocks[0]["code"]["rich_text"][0]["annotations"]["bold"], json!(false));
    }

    #[test]
    fn long_paragraphs_are_split_below_the_rich_text_limit() {
        let long = "字".repeat(5000);
        let blocks = markdown_to_blocks(&long);

        assert!(blocks.len() >= 3);
        for block in &blocks {
            let content = block["paragraph"]["rich_text"][0]["text"]["content"]
                .as_str()
                .unwrap();
            assert!(
                content.chars().count() <= MAX_RICH_TEXT_CHARS,
                "chunk was {} chars",
                content.chars().count()
            );
        }
    }

    #[test]
    fn transcripts_over_one_hundred_blocks_are_chunked_for_appending() {
        // A realistic long transcript: one bullet per segment.
        let markdown: String = (0..250).map(|i| format!("- segment {i}\n")).collect();
        let blocks = markdown_to_blocks(&markdown);
        assert_eq!(blocks.len(), 250);

        let chunks = chunk_blocks(blocks, MAX_BLOCKS_PER_REQUEST);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), 100);
        assert_eq!(chunks[1].len(), 100);
        assert_eq!(chunks[2].len(), 50);
    }

    #[test]
    fn chunking_handles_empty_and_exact_multiples() {
        assert!(chunk_blocks(Vec::new(), 100).is_empty());

        let blocks: Vec<Value> = (0..100)
            .map(|_| json!({ "object": "block", "type": "divider", "divider": {} }))
            .collect();
        let chunks = chunk_blocks(blocks, 100);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 100);
    }

    #[test]
    fn empty_markdown_produces_no_blocks() {
        assert!(markdown_to_blocks("").is_empty());
        assert!(markdown_to_blocks("\n\n").is_empty());
    }

    #[test]
    fn common_failures_produce_actionable_messages() {
        let unauthorized = notion_error_message(
            reqwest::StatusCode::UNAUTHORIZED,
            r#"{"object":"error","status":401,"code":"unauthorized","message":"API token is invalid."}"#,
            Some("0123456789abcdef0123456789abcdef"),
        );
        assert!(unauthorized.contains("token"));
        assert!(unauthorized.contains("Settings → Sync"));

        let not_found = notion_error_message(
            reqwest::StatusCode::NOT_FOUND,
            r#"{"object":"error","status":404,"code":"object_not_found","message":"Could not find database."}"#,
            Some("0123456789abcdef0123456789abcdef"),
        );
        assert!(not_found.contains("share the database"));
        assert!(not_found.contains("Connections"));

        let rate_limited = notion_error_message(reqwest::StatusCode::TOO_MANY_REQUESTS, "", None);
        assert!(rate_limited.contains("Wait a minute"));
    }
}
