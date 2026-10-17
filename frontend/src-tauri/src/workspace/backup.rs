//! Whole-library backup and restore.
//!
//! A backup is one zip:
//!
//! ```text
//! manifest.json            format version, counts, original roots, media map
//! database.sqlite          consistent snapshot via `VACUUM INTO`
//! dictionary.json          custom vocabulary
//! notes/…                  note images folder
//! media/<n>/…              (optional) each meeting/recording folder with audio
//! ```
//!
//! Restore never overwrites the live database while it is open. The snapshot is
//! staged next to it as `<db>.restore-pending`; on the next start
//! [`apply_pending_restore`] keeps the current file as a dated safety copy and
//! swaps the staged one in. Media folders are extracted under the current
//! recordings folder and every stored path is rewritten to the new location, so
//! a backup moves cleanly between machines.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tauri::{AppHandle, Manager, Runtime};
use zip::write::SimpleFileOptions;

use crate::state::AppState;

const FORMAT_VERSION: u32 = 1;
const MANIFEST: &str = "manifest.json";
const DB_ENTRY: &str = "database.sqlite";
const DICTIONARY_ENTRY: &str = "dictionary.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupManifest {
    pub format_version: u32,
    pub app_version: String,
    pub created_at: String,
    pub meetings: i64,
    pub transcripts: i64,
    pub include_audio: bool,
    pub notes_root: Option<String>,
    /// Original absolute folder → archive prefix (`media/<n>`).
    pub media: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackupResult {
    pub cancelled: bool,
    pub path: Option<String>,
    pub bytes: u64,
    pub meetings: i64,
    pub media_folders: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct RestoreResult {
    pub cancelled: bool,
    pub meetings: i64,
    pub created_at: Option<String>,
    pub requires_restart: bool,
}

pub fn pending_restore_path(db_path: &Path) -> PathBuf {
    let mut name = db_path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".restore-pending");
    db_path.with_file_name(name)
}

/// Swap a staged restore in before the pool opens. Called from DB startup.
pub fn apply_pending_restore(db_path: &Path) -> std::io::Result<bool> {
    let pending = pending_restore_path(db_path);
    if !pending.exists() {
        return Ok(false);
    }
    if db_path.exists() {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let safety = db_path.with_file_name(format!("meeting_minutes.before-restore-{stamp}.sqlite"));
        fs::rename(db_path, &safety)?;
        log::info!("Kept the previous database as {}", safety.display());
    }
    for suffix in ["-wal", "-shm"] {
        let mut side = db_path.as_os_str().to_os_string();
        side.push(suffix);
        let _ = fs::remove_file(PathBuf::from(side));
    }
    fs::rename(&pending, db_path)?;
    log::info!("Applied restored database at {}", db_path.display());
    Ok(true)
}

fn add_dir_to_zip<W: Write + std::io::Seek>(
    zip: &mut zip::ZipWriter<W>,
    dir: &Path,
    prefix: &str,
    options: SimpleFileOptions,
) -> Result<(), String> {
    if !dir.is_dir() {
        return Ok(());
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in fs::read_dir(&current).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let rel = path.strip_prefix(dir).map_err(|e| e.to_string())?;
            let name = format!("{prefix}/{}", rel.to_string_lossy().replace('\\', "/"));
            zip.start_file(name, options).map_err(|e| e.to_string())?;
            let mut f = File::open(&path).map_err(|e| e.to_string())?;
            std::io::copy(&mut f, zip).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

async fn media_folders(pool: &SqlitePool) -> Result<Vec<String>, sqlx::Error> {
    let mut folders: Vec<String> = sqlx::query_as::<_, (String,)>(
        "SELECT folder_path FROM meetings WHERE folder_path IS NOT NULL AND folder_path != ''
         UNION SELECT folder_path FROM recordings WHERE folder_path IS NOT NULL AND folder_path != ''",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(p,)| p)
    .filter(|p| Path::new(p).is_dir())
    .collect();
    folders.sort();
    folders.dedup();
    Ok(folders)
}

/// Write a backup zip to `target`.
pub async fn write_backup(
    pool: &SqlitePool,
    target: &Path,
    include_audio: bool,
    notes_root: Option<PathBuf>,
    dictionary_file: Option<PathBuf>,
    app_version: &str,
) -> Result<BackupResult, String> {
    let tmp_dir = std::env::temp_dir().join(format!("meetily-backup-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&tmp_dir).map_err(|e| e.to_string())?;
    let snapshot = tmp_dir.join(DB_ENTRY);
    // VACUUM INTO takes the file name as a literal; quote it ourselves.
    let vacuum = format!("VACUUM INTO '{}'", snapshot.to_string_lossy().replace('\'', "''"));
    sqlx::query(&vacuum)
        .execute(pool)
        .await
        .map_err(|e| format!("数据库快照失败：{e}"))?;

    let (meetings,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM meetings").fetch_one(pool).await.map_err(|e| e.to_string())?;
    let (transcripts,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM transcripts").fetch_one(pool).await.map_err(|e| e.to_string())?;
    let folders = if include_audio { media_folders(pool).await.map_err(|e| e.to_string())? } else { Vec::new() };
    let media: BTreeMap<String, String> = folders
        .iter()
        .enumerate()
        .map(|(i, f)| (f.clone(), format!("media/{i}")))
        .collect();
    let manifest = BackupManifest {
        format_version: FORMAT_VERSION,
        app_version: app_version.to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        meetings,
        transcripts,
        include_audio,
        notes_root: notes_root.as_ref().map(|p| p.to_string_lossy().to_string()),
        media: media.clone(),
    };
    let target = target.to_path_buf();
    let media_count = media.len();
    let written = tokio::task::spawn_blocking(move || -> Result<u64, String> {
        let file = File::create(&target).map_err(|e| format!("无法创建备份文件 {}：{e}", target.display()))?;
        let mut zip = zip::ZipWriter::new(file);
        let deflate = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        // Audio is already compressed; storing it avoids burning CPU for ~0%.
        let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file(MANIFEST, deflate).map_err(|e| e.to_string())?;
        zip.write_all(serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?.as_bytes())
            .map_err(|e| e.to_string())?;
        zip.start_file(DB_ENTRY, deflate).map_err(|e| e.to_string())?;
        let mut db = File::open(&snapshot).map_err(|e| format!("快照文件缺失 {}：{e}", snapshot.display()))?;
        std::io::copy(&mut db, &mut zip).map_err(|e| e.to_string())?;
        if let Some(dict) = dictionary_file.filter(|p| p.is_file()) {
            zip.start_file(DICTIONARY_ENTRY, deflate).map_err(|e| e.to_string())?;
            let mut f = File::open(dict).map_err(|e| e.to_string())?;
            std::io::copy(&mut f, &mut zip).map_err(|e| e.to_string())?;
        }
        if let Some(notes) = notes_root {
            add_dir_to_zip(&mut zip, &notes, "notes", deflate)?;
        }
        for (folder, prefix) in &media {
            add_dir_to_zip(&mut zip, Path::new(folder), prefix, stored)?;
        }
        zip.finish().map_err(|e| e.to_string())?;
        let _ = fs::remove_dir_all(snapshot.parent().unwrap_or(Path::new(".")));
        Ok(fs::metadata(&target).map(|m| m.len()).unwrap_or(0))
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(BackupResult { cancelled: false, path: None, bytes: written, meetings, media_folders: media_count })
}

/// Reject archive entries that would escape the extraction folder.
fn safe_relative(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.is_absolute() || name.contains("..") || name.contains(':') {
        return None;
    }
    Some(path.to_path_buf())
}

/// Read the manifest without extracting anything.
pub fn read_manifest(archive: &Path) -> Result<BackupManifest, String> {
    let file = File::open(archive).map_err(|e| format!("无法打开备份文件：{e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|_| "不是有效的 Meetily 备份文件".to_string())?;
    let mut raw = String::new();
    zip.by_name(MANIFEST)
        .map_err(|_| "备份文件缺少 manifest.json".to_string())?
        .read_to_string(&mut raw)
        .map_err(|e| e.to_string())?;
    let manifest: BackupManifest = serde_json::from_str(&raw).map_err(|e| format!("备份清单无法解析：{e}"))?;
    if manifest.format_version > FORMAT_VERSION {
        return Err("备份来自更新版本的 Meetily，请先升级应用".into());
    }
    Ok(manifest)
}

/// Extract an archive: database to `staged_db`, notes into `notes_root`,
/// media under `media_root`. Returns original folder → new folder.
pub fn extract_backup(
    archive: &Path,
    staged_db: &Path,
    notes_root: &Path,
    media_root: &Path,
    dictionary_out: &Path,
) -> Result<(BackupManifest, BTreeMap<String, String>), String> {
    let manifest = read_manifest(archive)?;
    let file = File::open(archive).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let prefix_to_new: BTreeMap<String, PathBuf> = manifest
        .media
        .iter()
        .map(|(orig, prefix)| {
            let base = Path::new(orig).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| prefix.replace('/', "-"));
            (prefix.clone(), media_root.join(base))
        })
        .collect();
    let mut found_db = false;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        let Some(rel) = safe_relative(&name) else {
            log::warn!("Skipping unsafe backup entry {name}");
            continue;
        };
        let out: PathBuf = if name == DB_ENTRY {
            found_db = true;
            staged_db.to_path_buf()
        } else if name == DICTIONARY_ENTRY {
            dictionary_out.to_path_buf()
        } else if let Ok(rest) = rel.strip_prefix("notes") {
            notes_root.join(rest)
        } else if name.starts_with("media/") {
            let mut parts = name.splitn(3, '/');
            let (_, n, rest) = (parts.next(), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
            match prefix_to_new.get(&format!("media/{n}")) {
                Some(dir) if !rest.is_empty() => dir.join(rest),
                _ => continue,
            }
        } else {
            continue;
        };
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut f = File::create(&out).map_err(|e| format!("写入 {} 失败：{e}", out.display()))?;
        std::io::copy(&mut entry, &mut f).map_err(|e| e.to_string())?;
    }
    if !found_db {
        return Err("备份文件中没有数据库".into());
    }
    let mapping = manifest
        .media
        .iter()
        .filter_map(|(orig, prefix)| prefix_to_new.get(prefix).map(|p| (orig.clone(), p.to_string_lossy().to_string())))
        .collect();
    Ok((manifest, mapping))
}

/// Point paths stored in the staged database at their restored locations.
pub async fn rewrite_paths(
    staged_db: &Path,
    folders: &BTreeMap<String, String>,
    old_notes_root: Option<&str>,
    new_notes_root: &str,
) -> Result<(), String> {
    let opts = sqlx::sqlite::SqliteConnectOptions::new().filename(staged_db);
    let pool = SqlitePool::connect_with(opts).await.map_err(|e| format!("无法打开备份数据库：{e}"))?;
    for (old, new) in folders {
        for sql in [
            "UPDATE meetings SET folder_path = ? WHERE folder_path = ?",
            "UPDATE recordings SET folder_path = ? WHERE folder_path = ?",
        ] {
            sqlx::query(sql).bind(new).bind(old).execute(&pool).await.map_err(|e| e.to_string())?;
        }
        sqlx::query("UPDATE recordings SET file_path = ? || substr(file_path, length(?) + 1) WHERE substr(file_path, 1, length(?)) = ?")
            .bind(new).bind(old).bind(old).bind(old)
            .execute(&pool).await.map_err(|e| e.to_string())?;
    }
    if let Some(old_notes) = old_notes_root.filter(|o| *o != new_notes_root) {
        sqlx::query("UPDATE meeting_notes SET notes_markdown = replace(notes_markdown, ?, ?), notes_json = replace(notes_json, ?, ?)")
            .bind(old_notes).bind(new_notes_root).bind(old_notes).bind(new_notes_root)
            .execute(&pool).await.map_err(|e| e.to_string())?;
    }
    pool.close().await;
    Ok(())
}

fn store_file<R: Runtime>(app: &AppHandle<R>, name: &str) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join(name))
}

#[tauri::command]
pub async fn api_create_backup<R: Runtime>(
    app: AppHandle<R>,
    include_audio: bool,
    state: tauri::State<'_, AppState>,
) -> Result<BackupResult, String> {
    use tauri_plugin_dialog::DialogExt;
    let name = format!("Meetily-备份-{}.zip", chrono::Local::now().format("%Y%m%d-%H%M"));
    let dialog_app = app.clone();
    let picked = tokio::task::spawn_blocking(move || {
        let mut builder = dialog_app.dialog().file().set_file_name(&name).add_filter("ZIP", &["zip"]);
        if let Some(dir) = dirs::document_dir().filter(|d| d.exists()) {
            builder = builder.set_directory(dir);
        }
        builder.blocking_save_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(target) = picked else {
        return Ok(BackupResult { cancelled: true, path: None, bytes: 0, meetings: 0, media_folders: 0 });
    };
    let target = PathBuf::from(target.to_string());
    let storage = crate::storage::config::load_storage_config(&app);
    let notes_root = storage.notes_folder(&app).ok();
    let version = app.package_info().version.to_string();
    let mut result = write_backup(
        state.db_manager.pool(),
        &target,
        include_audio,
        notes_root,
        store_file(&app, "dictionary.json"),
        &version,
    )
    .await?;
    result.path = Some(target.to_string_lossy().to_string());
    log::info!("Backup written to {} ({} bytes)", target.display(), result.bytes);
    Ok(result)
}

#[tauri::command]
pub async fn api_restore_backup<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
) -> Result<RestoreResult, String> {
    use tauri_plugin_dialog::DialogExt;
    let dialog_app = app.clone();
    let picked = tokio::task::spawn_blocking(move || {
        dialog_app.dialog().file().add_filter("Meetily 备份", &["zip"]).blocking_pick_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(archive) = picked else {
        return Ok(RestoreResult { cancelled: true, meetings: 0, created_at: None, requires_restart: false });
    };
    let archive = PathBuf::from(archive.to_string());

    let db_path = state.db_manager.db_path().to_path_buf();
    let staged = pending_restore_path(&db_path);
    let storage = crate::storage::config::load_storage_config(&app);
    let notes_root = storage.notes_folder(&app)?;
    let media_root = crate::audio::recording_preferences::load_recording_preferences(&app)
        .await
        .map(|p| p.save_folder)
        .unwrap_or_else(|_| crate::audio::recording_preferences::get_default_recordings_folder())
        .join(format!("restored-{}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    let dict_tmp = std::env::temp_dir().join(format!("meetily-dict-{}.json", uuid::Uuid::new_v4()));

    let (staged_c, notes_c, media_c, dict_c, archive_c) =
        (staged.clone(), notes_root.clone(), media_root.clone(), dict_tmp.clone(), archive.clone());
    let (manifest, mapping) = tokio::task::spawn_blocking(move || {
        extract_backup(&archive_c, &staged_c, &notes_c, &media_c, &dict_c)
    })
    .await
    .map_err(|e| e.to_string())??;

    if let Err(e) = rewrite_paths(&staged, &mapping, manifest.notes_root.as_deref(), &notes_root.to_string_lossy()).await {
        let _ = fs::remove_file(&staged);
        return Err(e);
    }

    // Merge the vocabulary instead of replacing it: the user may have added
    // terms on this machine since the backup was taken.
    if let Ok(raw) = fs::read_to_string(&dict_tmp) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) {
            if let Some(entries) = value.get("dictionary").cloned() {
                if let Ok(restored) = serde_json::from_value::<crate::dictionary::UserDictionary>(entries) {
                    let mut current = crate::dictionary::load_dictionary(&app);
                    current.entries.extend(restored.entries);
                    let _ = crate::dictionary::save_dictionary(&app, &current);
                }
            }
        }
        let _ = fs::remove_file(&dict_tmp);
    }

    Ok(RestoreResult {
        cancelled: false,
        meetings: manifest.meetings,
        created_at: Some(manifest.created_at),
        requires_restart: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::search::tests::seed;

    #[test]
    fn unsafe_entries_are_rejected() {
        assert!(safe_relative("../etc/passwd").is_none());
        assert!(safe_relative("/abs").is_none());
        assert!(safe_relative("C:/x").is_none());
        assert!(safe_relative("media/0/audio.mp4").is_some());
    }

    #[tokio::test]
    async fn backup_round_trip_restores_db_media_and_paths() {
        let root = std::env::temp_dir().join(format!("meetily-bk-test-{}", uuid::Uuid::new_v4()));
        let rec = root.join("old-recordings").join("Meeting A");
        fs::create_dir_all(&rec).unwrap();
        fs::write(rec.join("audio.mp4"), b"fake-audio").unwrap();
        let notes = root.join("old-notes");
        fs::create_dir_all(&notes).unwrap();
        fs::write(notes.join("img.png"), b"png").unwrap();

        // A file-backed database, like production (VACUUM INTO needs one).
        let db_file = root.join("live.sqlite");
        let opts = sqlx::sqlite::SqliteConnectOptions::new().filename(&db_file).create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        seed(&pool).await;
        sqlx::query("UPDATE meetings SET folder_path = ? WHERE id = 'm1'")
            .bind(rec.to_string_lossy().to_string())
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO meeting_notes (meeting_id, notes_markdown, notes_json, created_at, updated_at) VALUES ('m1', ?, '{}', 'x', 'x')")
            .bind(format!("![img]({}/img.png)", notes.to_string_lossy()))
            .execute(&pool).await.unwrap();

        let zip_path = root.join("backup.zip");
        let result = write_backup(&pool, &zip_path, true, Some(notes.clone()), None, "test").await.unwrap();
        assert_eq!(result.meetings, 2);
        assert_eq!(result.media_folders, 1);

        let manifest = read_manifest(&zip_path).unwrap();
        assert!(manifest.include_audio);

        let staged = root.join("restore").join("db.sqlite.restore-pending");
        let new_notes = root.join("new-notes");
        let media_root = root.join("new-recordings");
        let (manifest, mapping) = extract_backup(&zip_path, &staged, &new_notes, &media_root, &root.join("dict.json")).unwrap();
        assert!(new_notes.join("img.png").is_file());
        let new_folder = media_root.join("Meeting A");
        assert!(new_folder.join("audio.mp4").is_file());

        rewrite_paths(&staged, &mapping, manifest.notes_root.as_deref(), &new_notes.to_string_lossy()).await.unwrap();
        let restored = SqlitePool::connect(&format!("sqlite://{}", staged.to_string_lossy())).await.unwrap();
        let (folder,): (String,) = sqlx::query_as("SELECT folder_path FROM meetings WHERE id = 'm1'").fetch_one(&restored).await.unwrap();
        assert_eq!(folder, new_folder.to_string_lossy());
        let (md,): (String,) = sqlx::query_as("SELECT notes_markdown FROM meeting_notes WHERE meeting_id = 'm1'").fetch_one(&restored).await.unwrap();
        assert!(md.contains(&*new_notes.to_string_lossy()));
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM transcripts").fetch_one(&restored).await.unwrap();
        assert_eq!(n, 4);
        restored.close().await;

        // Startup swap keeps a safety copy of the current DB.
        let live = root.join("restore").join("db.sqlite");
        fs::write(&live, b"old").unwrap();
        assert!(apply_pending_restore(&live).unwrap());
        assert!(!staged.exists());
        let safety = fs::read_dir(root.join("restore")).unwrap().filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().contains("before-restore"));
        assert!(safety);
        let _ = fs::remove_dir_all(&root);
    }
}
