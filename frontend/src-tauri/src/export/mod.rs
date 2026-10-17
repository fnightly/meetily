//! Writes meeting exports to disk.
//!
//! The documents themselves (Markdown, text, HTML, subtitles, JSON and the
//! WordprocessingML parts of a .docx) are rendered by the frontend
//! (`src/lib/export-formats.ts`), where the summary representations already
//! live. This module only owns what a webview cannot do reliably: native
//! save/folder dialogs, writing files, and packaging .docx parts into a zip.
//! (`<a download>` on a blob URL silently does nothing in some webviews.)

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Runtime};

#[derive(Debug, Clone, Deserialize)]
pub struct DocxPart {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ExportFile {
    Text { name: String, text: String },
    Docx { name: String, parts: Vec<DocxPart> },
}

impl ExportFile {
    fn name(&self) -> &str {
        match self {
            ExportFile::Text { name, .. } | ExportFile::Docx { name, .. } => name,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportResult {
    pub paths: Vec<String>,
    pub folder: Option<String>,
    pub cancelled: bool,
}

/// Keep a file name inside the chosen folder and valid on every OS.
pub fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').trim();
    if trimmed.is_empty() {
        "export".to_string()
    } else {
        trimmed.chars().take(120).collect()
    }
}

/// `name.ext`, then `name (2).ext`, ... — never overwrite a file written in the
/// same batch or one that already exists in the folder.
pub fn unique_path(folder: &Path, name: &str, taken: &mut std::collections::HashSet<PathBuf>) -> PathBuf {
    let candidate = folder.join(name);
    if !candidate.exists() && !taken.contains(&candidate) {
        taken.insert(candidate.clone());
        return candidate;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(idx) if idx > 0 => (&name[..idx], &name[idx..]),
        _ => (name, ""),
    };
    for n in 2..10_000 {
        let p = folder.join(format!("{} ({}){}", stem, n, ext));
        if !p.exists() && !taken.contains(&p) {
            taken.insert(p.clone());
            return p;
        }
    }
    candidate
}

/// Package WordprocessingML parts into a .docx (a zip with a fixed layout).
/// `[Content_Types].xml` must be the first entry for some readers.
pub fn write_docx(path: &Path, parts: &[DocxPart]) -> Result<(), String> {
    if !parts.iter().any(|p| p.path == "word/document.xml") {
        return Err("Invalid Word document: word/document.xml is missing".into());
    }
    let file = std::fs::File::create(path).map_err(|e| format!("Cannot create {}: {}", path.display(), e))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let mut ordered: Vec<&DocxPart> = parts.iter().collect();
    ordered.sort_by_key(|p| if p.path == "[Content_Types].xml" { 0 } else { 1 });
    for part in ordered {
        if part.path.contains("..") || part.path.starts_with('/') {
            return Err(format!("Invalid document part path: {}", part.path));
        }
        zip.start_file(part.path.as_str(), options)
            .map_err(|e| format!("Failed to write {}: {}", part.path, e))?;
        zip.write_all(part.content.as_bytes())
            .map_err(|e| format!("Failed to write {}: {}", part.path, e))?;
    }
    zip.finish().map_err(|e| format!("Failed to finish Word document: {}", e))?;
    Ok(())
}

fn write_one(path: &Path, file: &ExportFile) -> Result<(), String> {
    match file {
        ExportFile::Text { text, .. } => {
            // UTF-8 BOM for plain text / subtitles so Windows apps (Notepad, Excel,
            // many players) don't misread Chinese as ANSI.
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            let mut bytes = Vec::with_capacity(text.len() + 3);
            if matches!(ext.as_str(), "txt" | "srt") {
                bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
            }
            bytes.extend_from_slice(text.as_bytes());
            std::fs::write(path, bytes).map_err(|e| format!("Cannot write {}: {}", path.display(), e))
        }
        ExportFile::Docx { parts, .. } => write_docx(path, parts),
    }
}

/// Write files into `folder`, returning the paths written.
pub fn write_files_to_folder(folder: &Path, files: &[ExportFile]) -> Result<Vec<PathBuf>, String> {
    std::fs::create_dir_all(folder).map_err(|e| format!("Cannot create {}: {}", folder.display(), e))?;
    let mut taken = std::collections::HashSet::new();
    let mut written = Vec::with_capacity(files.len());
    for file in files {
        let path = unique_path(folder, &sanitize_file_name(file.name()), &mut taken);
        write_one(&path, file)?;
        written.push(path);
    }
    Ok(written)
}

/// Save exported documents.
///
/// `mode = "file"` (single file): native "Save as" dialog.
/// `mode = "folder"` (batch): pick a folder, every file is written into it.
/// Returns `cancelled: true` when the user dismisses the dialog.
#[tauri::command]
pub async fn export_save_files<R: Runtime>(
    app: AppHandle<R>,
    files: Vec<ExportFile>,
    mode: Option<String>,
    dialog_title: Option<String>,
) -> Result<ExportResult, String> {
    use tauri_plugin_dialog::DialogExt;

    if files.is_empty() {
        return Err("Nothing to export".into());
    }
    let single = files.len() == 1 && mode.as_deref() != Some("folder");
    let default_dir = dirs::document_dir().or_else(dirs::home_dir);

    if single {
        let name = sanitize_file_name(files[0].name());
        let ext = Path::new(&name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();
        let app_for_dialog = app.clone();
        let title = dialog_title.clone();
        let picked = tokio::task::spawn_blocking(move || {
            let mut builder = app_for_dialog.dialog().file().set_file_name(&name);
            if let Some(t) = title {
                builder = builder.set_title(t);
            }
            if let Some(dir) = default_dir.filter(|d| d.exists()) {
                builder = builder.set_directory(dir);
            }
            if !ext.is_empty() {
                builder = builder.add_filter(ext.to_uppercase(), &[ext.as_str()]);
            }
            builder.blocking_save_file()
        })
        .await
        .map_err(|e| format!("Save dialog failed: {}", e))?;

        let Some(target) = picked else {
            return Ok(ExportResult { paths: vec![], folder: None, cancelled: true });
        };
        let path = PathBuf::from(target.to_string());
        let file = files.into_iter().next().unwrap();
        let path_for_write = path.clone();
        tokio::task::spawn_blocking(move || write_one(&path_for_write, &file))
            .await
            .map_err(|e| format!("Export task failed: {}", e))??;
        log::info!("Exported meeting to {}", path.display());
        return Ok(ExportResult {
            folder: path.parent().map(|p| p.to_string_lossy().to_string()),
            paths: vec![path.to_string_lossy().to_string()],
            cancelled: false,
        });
    }

    let app_for_dialog = app.clone();
    let picked = tokio::task::spawn_blocking(move || {
        let mut builder = app_for_dialog.dialog().file();
        if let Some(t) = dialog_title {
            builder = builder.set_title(t);
        }
        if let Some(dir) = default_dir.filter(|d| d.exists()) {
            builder = builder.set_directory(dir);
        }
        builder.blocking_pick_folder()
    })
    .await
    .map_err(|e| format!("Folder dialog failed: {}", e))?;

    let Some(folder) = picked else {
        return Ok(ExportResult { paths: vec![], folder: None, cancelled: true });
    };
    let folder = PathBuf::from(folder.to_string());
    let folder_for_write = folder.clone();
    let written = tokio::task::spawn_blocking(move || write_files_to_folder(&folder_for_write, &files))
        .await
        .map_err(|e| format!("Export task failed: {}", e))??;
    log::info!("Exported {} file(s) to {}", written.len(), folder.display());
    Ok(ExportResult {
        paths: written.iter().map(|p| p.to_string_lossy().to_string()).collect(),
        folder: Some(folder.to_string_lossy().to_string()),
        cancelled: false,
    })
}

/// Show an exported file (or folder) in the OS file manager.
#[tauri::command]
pub async fn reveal_export_path(path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err(format!("{} no longer exists", path));
    }
    #[cfg(target_os = "windows")]
    let result = if p.is_file() {
        std::process::Command::new("explorer").arg(format!("/select,{}", path)).spawn()
    } else {
        std::process::Command::new("explorer").arg(&path).spawn()
    };
    #[cfg(target_os = "macos")]
    let result = if p.is_file() {
        std::process::Command::new("open").arg("-R").arg(&path).spawn()
    } else {
        std::process::Command::new("open").arg(&path).spawn()
    };
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let result = {
        let target = if p.is_file() { p.parent().map(|d| d.to_path_buf()).unwrap_or(p.clone()) } else { p.clone() };
        std::process::Command::new("xdg-open").arg(target).spawn()
    };
    result.map(|_| ()).map_err(|e| format!("Failed to open file manager: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("meetily-export-test-{}-{}", tag, uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn file_names_are_made_safe() {
        assert_eq!(sanitize_file_name("周会: Q3/规划?.md"), "周会- Q3-规划-.md");
        assert_eq!(sanitize_file_name("  ..  "), "export");
        assert_eq!(sanitize_file_name("a\u{0007}b.txt"), "a-b.txt");
    }

    #[test]
    fn duplicate_names_get_a_counter() {
        let dir = tmp_dir("dup");
        let files = vec![
            ExportFile::Text { name: "会议.md".into(), text: "a".into() },
            ExportFile::Text { name: "会议.md".into(), text: "b".into() },
        ];
        let written = write_files_to_folder(&dir, &files).unwrap();
        assert_eq!(written[0].file_name().unwrap(), "会议.md");
        assert_eq!(written[1].file_name().unwrap(), "会议 (2).md");
        // A second export never overwrites the first.
        let again = write_files_to_folder(&dir, &files[..1]).unwrap();
        assert_eq!(again[0].file_name().unwrap(), "会议 (3).md");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn text_exports_add_bom_only_where_it_helps() {
        let dir = tmp_dir("bom");
        let files = vec![
            ExportFile::Text { name: "a.txt".into(), text: "中文".into() },
            ExportFile::Text { name: "a.md".into(), text: "中文".into() },
        ];
        let written = write_files_to_folder(&dir, &files).unwrap();
        assert_eq!(&std::fs::read(&written[0]).unwrap()[..3], &[0xEF, 0xBB, 0xBF]);
        assert_eq!(std::fs::read_to_string(&written[1]).unwrap(), "中文");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn docx_is_a_zip_with_content_types_first() {
        let dir = tmp_dir("docx");
        let parts = vec![
            DocxPart { path: "word/document.xml".into(), content: "<w:document/>".into() },
            DocxPart { path: "[Content_Types].xml".into(), content: "<Types/>".into() },
        ];
        let written = write_files_to_folder(&dir, &[ExportFile::Docx { name: "x.docx".into(), parts }]).unwrap();
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&written[0]).unwrap()).unwrap();
        assert_eq!(archive.by_index(0).unwrap().name(), "[Content_Types].xml");
        let mut body = String::new();
        archive.by_name("word/document.xml").unwrap().read_to_string(&mut body).unwrap();
        assert_eq!(body, "<w:document/>");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn docx_rejects_bad_parts() {
        let dir = tmp_dir("bad");
        let missing = write_docx(&dir.join("a.docx"), &[DocxPart { path: "x.xml".into(), content: String::new() }]);
        assert!(missing.is_err());
        let escape = write_docx(
            &dir.join("b.docx"),
            &[
                DocxPart { path: "word/document.xml".into(), content: String::new() },
                DocxPart { path: "../evil.xml".into(), content: String::new() },
            ],
        );
        assert!(escape.is_err());
        std::fs::remove_dir_all(dir).ok();
    }
}
