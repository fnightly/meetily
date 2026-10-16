// audio/diarization_commands.rs
//
// Model status and download for speaker separation.
//
// The models are fetched rather than bundled: together they are ~45 MB, most of it the
// speaker-embedding model, which is more than the rest of the app's own code and would
// bloat every install including those that never enable separation.
//
// Downloads go through the proxy-aware client and are checked for free space first, and
// each file is written to a temporary name and only moved into place once its size
// matches. An interrupted download therefore leaves no file that looks usable — which
// matters because a truncated ONNX file fails much later, inside inference, with an
// error that says nothing about the real cause.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use super::diarization_service::DiarizationPhase;

use super::diarization_catalog as catalog;

/// Suffix used while a download is in flight.
const PARTIAL_SUFFIX: &str = ".partial";

/// What the UI needs to know about the diarisation models.
#[derive(Debug, Clone, Serialize)]
pub struct DiarizationModelStatus {
    /// True when both models are present at their expected sizes.
    pub ready: bool,
    /// Directory the models live in.
    pub folder: String,
    /// Filenames that are absent or the wrong size.
    pub missing: Vec<String>,
    /// Human-readable description, safe to show directly.
    pub message: String,
    /// Bytes a full download needs.
    pub required_bytes: u64,
    /// Bytes free where the models would be written, when known.
    pub available_bytes: Option<u64>,
    /// False when there is not enough room; the download refuses rather than filling
    /// the disk.
    pub has_room: bool,
}

/// Resolve the diarisation model directory, creating nothing.
fn resolve_models_dir<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    let base = match crate::storage::load_storage_config(app).models_folder(app) {
        Ok(dir) => dir,
        Err(error) => {
            log::warn!("Could not resolve the configured models folder ({error}); using the default");
            crate::storage::config::default_models_dir(app)?
        }
    };

    Ok(base.join("diarization"))
}

/// Build the status for `dir`.
fn status_for(dir: &Path) -> DiarizationModelStatus {
    let missing = catalog::missing_artifacts(dir);
    let required_bytes: u64 = missing.iter().map(|artifact| artifact.exact_bytes).sum();

    let available = crate::disk_space::available_space_bytes(dir);
    // A margin over the download size, so the extraction-free write plus the rest of
    // the app still has room. Matches the guard used for the transcription models.
    let has_room = available
        .map(|free| crate::whisper_engine::download::has_room_for(free, required_bytes))
        .unwrap_or(true);

    DiarizationModelStatus {
        ready: missing.is_empty(),
        folder: dir.display().to_string(),
        missing: missing.iter().map(|artifact| artifact.filename.to_string()).collect(),
        message: catalog::missing_summary(dir),
        required_bytes,
        available_bytes: available,
        has_room,
    }
}

/// Report whether speaker separation can run, and what it would need.
#[tauri::command]
pub async fn get_diarization_model_status<R: Runtime>(
    app: AppHandle<R>,
) -> Result<DiarizationModelStatus, String> {
    let dir = resolve_models_dir(&app)?;
    Ok(status_for(&dir))
}

/// Download any missing diarisation model.
///
/// Returns the resulting status, so the caller can render the same shape whether the
/// models were already present or just fetched.
#[tauri::command]
pub async fn download_diarization_models<R: Runtime>(
    app: AppHandle<R>,
) -> Result<DiarizationModelStatus, String> {
    let dir = resolve_models_dir(&app)?;

    let missing = catalog::missing_artifacts(&dir);
    if missing.is_empty() {
        log::info!("Diarisation models are already present in {}", dir.display());
        return Ok(status_for(&dir));
    }

    let needed: u64 = missing.iter().map(|artifact| artifact.exact_bytes).sum();

    // Refuse before writing anything rather than filling the disk part way through.
    match crate::disk_space::available_space_bytes(&dir) {
        Some(free) if !crate::whisper_engine::download::has_room_for(free, needed) => {
            return Err(format!(
                "Not enough disk space for the speaker separation models: {} MB needed, {} MB free in {}",
                crate::disk_space::human_mb(needed),
                crate::disk_space::human_mb(free),
                dir.display()
            ));
        }
        _ => {}
    }

    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Could not create {}: {e}", dir.display()))?;

    let client = crate::http_client::client_with_proxy();

    for artifact in missing {
        download_one(&client, &dir, artifact).await?;
    }

    log::info!("Diarisation models downloaded into {}", dir.display());
    Ok(status_for(&dir))
}

/// Download one artifact into `dir`, verifying its size before it counts as present.
async fn download_one(
    client: &reqwest::Client,
    dir: &Path,
    artifact: &'static catalog::DiarizationArtifact,
) -> Result<(), String> {
    let final_path = catalog::artifact_path(dir, artifact);
    let partial_path = dir.join(format!("{}{PARTIAL_SUFFIX}", artifact.filename));
    let url = catalog::download_url(artifact);

    log::info!(
        "Downloading {} ({} MB) from {}",
        artifact.filename,
        artifact.exact_bytes / 1_048_576,
        url
    );

    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Failed to start the download of {}: {e}", artifact.filename))?;

    if !response.status().is_success() {
        return Err(format!(
            "Downloading {} failed with HTTP {}",
            artifact.filename,
            response.status()
        ));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("Failed while downloading {}: {e}", artifact.filename))?;

    // Check before writing: a truncated body would otherwise be stored and only fail
    // later, inside ONNX, with an error that hides the cause.
    if bytes.len() as u64 != artifact.exact_bytes {
        // Clear any earlier partial so a retry starts clean.
        let _ = std::fs::remove_file(&partial_path);
        return Err(format!(
            "{} downloaded {} bytes but {} were expected; the download was incomplete",
            artifact.filename,
            bytes.len(),
            artifact.exact_bytes
        ));
    }

    std::fs::write(&partial_path, &bytes)
        .map_err(|e| format!("Could not write {}: {e}", partial_path.display()))?;

    // Move into place only now, so a file at the final path is always complete.
    if final_path.exists() {
        std::fs::remove_file(&final_path)
            .map_err(|e| format!("Could not replace {}: {e}", final_path.display()))?;
    }

    std::fs::rename(&partial_path, &final_path).map_err(|e| {
        format!(
            "Could not move {} into place at {}: {e}",
            partial_path.display(),
            final_path.display()
        )
    })?;

    log::info!("Downloaded {} to {}", artifact.filename, final_path.display());
    Ok(())
}

/// Open the diarisation models folder in the system file browser.
///
/// Creates it first: the folder does not exist until a download runs, and opening a
/// missing path fails.
#[tauri::command]
pub async fn open_diarization_models_folder<R: Runtime>(
    app: AppHandle<R>,
) -> Result<(), String> {
    let dir = resolve_models_dir(&app)?;

    if !dir.exists() {
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    }

    let path = dir.to_string_lossy().to_string();

    #[cfg(target_os = "windows")]
    std::process::Command::new("explorer")
        .arg(&path)
        .spawn()
        .map_err(|e| format!("Could not open {path}: {e}"))?;

    #[cfg(target_os = "macos")]
    std::process::Command::new("open")
        .arg(&path)
        .spawn()
        .map_err(|e| format!("Could not open {path}: {e}"))?;

    #[cfg(all(unix, not(target_os = "macos")))]
    std::process::Command::new("xdg-open")
        .arg(&path)
        .spawn()
        .map_err(|e| format!("Could not open {path}: {e}"))?;

    Ok(())
}

/// The directory the diarisation models live in, for the UI to reveal.
#[tauri::command]
pub async fn get_diarization_models_folder<R: Runtime>(
    app: AppHandle<R>,
) -> Result<String, String> {
    Ok(resolve_models_dir(&app)?.display().to_string())
}


// ---------------------------------------------------------------------------
// Running separation over a finished meeting
// ---------------------------------------------------------------------------

/// One transcript row's timing, read from the database.
#[derive(Debug, Clone)]
pub(crate) struct TranscriptRow {
    pub(crate) id: String,
    pub(crate) audio_start_time: Option<f64>,
    pub(crate) audio_end_time: Option<f64>,
}

/// What labelling a meeting produced.
#[derive(Debug, Clone)]
pub(crate) struct LabelOutcome {
    /// Segments that received a speaker.
    pub labelled: usize,
    /// Speaker indices that received at least one segment, in order.
    ///
    /// This is what the user ends up seeing, and it can be fewer than the number of
    /// speakers separation found: a speaker whose total speech is a fraction of a second
    /// can never win a segment against longer turns, and reporting the larger number would
    /// promise a speaker the transcript does not contain.
    pub speakers: std::collections::BTreeSet<u32>,
}

/// Attribute `turns` to a meeting's transcript segments and persist the labels.
///
/// Extracted from the command so the write path can be tested against a real database
/// without loading the ONNX models. Producing the turns and turning them into rows are
/// separate concerns, and only the second one needs a database.
///
/// Rows without both timings are skipped rather than guessed at: they cannot be placed on
/// the timeline, and a defaulted label would be wrong for the whole meeting.
///
/// Returns the number of segments labelled.
pub(crate) async fn apply_speaker_labels(
    pool: &sqlx::SqlitePool,
    rows: &[TranscriptRow],
    turns: &[super::speaker_turns::SpeakerTurn],
) -> Result<LabelOutcome, String> {
    let placeable: Vec<&TranscriptRow> = rows
        .iter()
        .filter(|row| row.audio_start_time.is_some() && row.audio_end_time.is_some())
        .collect();

    if placeable.is_empty() {
        return Ok(LabelOutcome { labelled: 0, speakers: Default::default() });
    }

    let spans: Vec<super::speaker_turns::TranscriptSpan> = placeable
        .iter()
        .map(|row| super::speaker_turns::TranscriptSpan {
            start: row.audio_start_time.unwrap_or(0.0),
            end: row.audio_end_time.unwrap_or(0.0),
        })
        .collect();

    let attributions = super::speaker_turns::attribute_segments(&spans, turns);

    let mut conn = pool
        .acquire()
        .await
        .map_err(|e| format!("Database error: {e}"))?;
    let mut tx = sqlx::Connection::begin(&mut *conn)
        .await
        .map_err(|e| format!("Failed to start a transaction: {e}"))?;

    let mut labelled = 0usize;
    let mut speakers: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();

    for (row, attribution) in placeable.iter().zip(attributions.iter()) {
        let Some(speaker) = attribution.speaker else {
            continue;
        };

        // Stored as "speaker_1" so the label is language-neutral and sortable; the
        // interface renders it however it wants.
        let label = format!("speaker_{}", speaker + 1);

        sqlx::query("UPDATE transcripts SET speaker = ? WHERE id = ?")
            .bind(&label)
            .bind(&row.id)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("Failed to label transcript {}: {e}", row.id))?;

        speakers.insert(speaker);
        labelled += 1;
    }

    tx.commit()
        .await
        .map_err(|e| format!("Failed to commit speaker labels: {e}"))?;

    Ok(LabelOutcome { labelled, speakers })
}

/// Read the timings separation needs to attribute speakers.
///
/// Only rows with both timings can be attributed: a row without them cannot be placed on
/// the timeline, so it keeps whatever label it has rather than being guessed at.
async fn load_transcript_rows(
    pool: &sqlx::SqlitePool,
    meeting_id: &str,
) -> Result<Vec<TranscriptRow>, String> {
    let rows: Vec<(String, Option<f64>, Option<f64>)> = sqlx::query_as(
        "SELECT id, audio_start_time, audio_end_time FROM transcripts WHERE meeting_id = ? ORDER BY audio_start_time",
    )
    .bind(meeting_id)
    .fetch_all(pool)
    .await
    .map_err(|e| format!("Failed to read transcripts for {meeting_id}: {e}"))?;

    Ok(rows
        .into_iter()
        .map(|(id, start, end)| TranscriptRow {
            id,
            audio_start_time: start,
            audio_end_time: end,
        })
        .collect())
}

/// Event carrying separation progress, so a long run does not look hung.
pub const DIARIZATION_PROGRESS_EVENT: &str = "diarization-progress";

/// How far a separation run has got.
#[derive(Debug, Clone, Serialize)]
pub struct DiarizationProgress {
    pub meeting_id: String,
    pub phase: DiarizationPhase,
    /// Whole percent, 0-100, within the current phase.
    pub percent: i32,
    /// Caption for the phase, already localised to English; the interface may translate
    /// using `phase` instead.
    pub label: &'static str,
}

/// What a separation run did, for the interface to report.
#[derive(Debug, Clone, Serialize)]
pub struct DiarizationRunResult {
    pub meeting_id: String,
    /// Distinct speakers found.
    pub speaker_count: usize,
    /// Segments given a speaker label.
    pub labelled_segments: usize,
    /// Segments in the meeting, including any that could not be placed on the timeline.
    pub total_segments: usize,
    pub duration_seconds: f64,
    pub message: String,
}

/// Assign a speaker to every transcript segment of a meeting.
///
/// Writes only the `speaker` column: text, timings and ids are left alone, so separation
/// can be re-run without disturbing anything else and the audio need not be re-transcribed.
///
/// The audio is located inside `meeting_folder_path`, so the caller does not need to know
/// which container the recording was written in.
///
/// Zero labelled segments is a normal outcome for a recording with nothing separable, not
/// an error.
#[tauri::command]
pub async fn diarize_meeting<R: Runtime>(
    app: AppHandle<R>,
    meeting_id: String,
    meeting_folder_path: String,
) -> Result<DiarizationRunResult, String> {
    let dir = resolve_models_dir(&app)?;

    if !catalog::is_ready(&dir) {
        return Err(catalog::missing_summary(&dir));
    }

    let state = app
        .try_state::<crate::state::AppState>()
        .ok_or_else(|| "Application state is unavailable".to_string())?;
    let pool = state.db_manager.pool();

    let rows = load_transcript_rows(pool, &meeting_id).await?;
    let placeable: Vec<&TranscriptRow> = rows
        .iter()
        .filter(|row| row.audio_start_time.is_some() && row.audio_end_time.is_some())
        .collect();

    if placeable.is_empty() {
        return Err(
            "This meeting has no transcript segments with timing information, so speakers \
             cannot be matched to them"
                .to_string(),
        );
    }

    // Take the folder rather than a filename, matching `start_retranscription_command`:
    // the caller knows the meeting, not which extension this recording happens to use.
    let folder = std::path::PathBuf::from(&meeting_folder_path);
    let path = super::recordings::find_audio_file(&folder).ok_or_else(|| {
        format!(
            "No recording was found in {} to separate speakers from",
            folder.display()
        )
    })?;

    // Decoding and resampling are CPU-bound, so they run off the async runtime, matching
    // how retranscription does it.
    let samples = tokio::task::spawn_blocking(move || -> Result<Vec<f32>, String> {
        let decoded = crate::audio::decoder::decode_audio_file(&path)
            .map_err(|e| format!("Failed to decode {}: {e}", path.display()))?;
        Ok(decoded.to_whisper_format())
    })
    .await
    .map_err(|e| format!("Decode task failed: {e}"))??;

    log::info!(
        "Separating speakers for meeting {meeting_id} over {} samples",
        samples.len()
    );

    // The ONNX work is blocking too, and can take minutes on a long meeting. Progress is
    // emitted as an event rather than returned, because the call does not finish until the
    // run does and the interface needs to say something before then.
    let models_dir = dir.clone();
    let progress_app = app.clone();
    let progress_meeting = meeting_id.clone();

    // The tunables are read here rather than inside the blocking task, so a store failure
    // cannot surface after the models have already been loaded.
    let separation_config =
        super::diarization_preferences::load_diarization_preferences(&app).to_config();

    log::info!(
        "Separating with threshold {:.2} and a speaker bound of {} (calibrated default is {:.2})",
        separation_config.threshold,
        separation_config.max_speakers,
        super::speaker_cluster::CALIBRATED_THRESHOLD
    );

    let outcome = tokio::task::spawn_blocking(move || {
        // A cell rather than a mutable binding: the callback signature is `Fn`, and the
        // run happens on one blocking thread, so there is no sharing to guard against.
        let last_reported = std::cell::RefCell::new(None::<(DiarizationPhase, i32)>);

        let report = |phase: DiarizationPhase, fraction: f32| {
            // Segmentation reports once per batch, which for a long meeting is hundreds of
            // events. Send one only when the whole percent changes, so the interface is not
            // woken for updates it cannot show.
            let percent = (fraction * 100.0).round() as i32;
            if *last_reported.borrow() == Some((phase, percent)) {
                return;
            }
            *last_reported.borrow_mut() = Some((phase, percent));
            let payload = DiarizationProgress {
                meeting_id: progress_meeting.clone(),
                phase,
                percent,
                label: phase.label(),
            };

            if let Err(error) = progress_app.emit(DIARIZATION_PROGRESS_EVENT, payload) {
                // A closed window is not a reason to abandon a run that is already going.
                log::debug!("Could not emit separation progress: {error}");
            }
        };

        super::diarization_service::diarize_samples_with_progress(
            &samples,
            &models_dir,
            separation_config,
            Some(&report),
        )
    })
    .await
    .map_err(|e| format!("Speaker separation task failed: {e}"))??;

    if outcome.is_empty() {
        log::info!("No separable speakers found for meeting {meeting_id}");
        return Ok(DiarizationRunResult {
            meeting_id,
            speaker_count: 0,
            labelled_segments: 0,
            total_segments: rows.len(),
            duration_seconds: outcome.duration_seconds,
            message: "No distinct speakers could be identified in this recording".to_string(),
        });
    }

    // Attribute each segment to the speaker it overlaps most, then write the labels.
    let labels = apply_speaker_labels(pool, &rows, &outcome.turns).await?;
    let labelled = labels.labelled;
    let labelled_speakers = labels.speakers.len();

    // Report the speakers that reached the transcript, not the clusters separation found.
    // A speaker whose total speech is a fraction of a second can own no segment, and
    // announcing them would promise a speaker the transcript does not contain.
    if labelled_speakers < outcome.speaker_count {
        log::info!(
            "{} speaker(s) found but only {labelled_speakers} could be matched to a segment",
            outcome.speaker_count
        );
    }

    log::info!(
        "Labelled {labelled} of {} segment(s) for meeting {meeting_id} across {labelled_speakers} speaker(s)",
        rows.len()
    );

    Ok(DiarizationRunResult {
        meeting_id,
        speaker_count: labelled_speakers,
        labelled_segments: labelled,
        total_segments: rows.len(),
        duration_seconds: outcome.duration_seconds,
        message: format!("Identified {labelled_speakers} speaker(s) across {labelled} segment(s)"),
    })
}

/// The tunables and the bands they may take, for the settings panel.
#[derive(Debug, Clone, Serialize)]
pub struct DiarizationPreferenceView {
    pub preferences: super::diarization_preferences::DiarizationPreferences,
    pub bounds: super::diarization_preferences::DiarizationPreferenceBounds,
    /// Whether these are still the calibrated defaults.
    pub is_default: bool,
}

fn preference_view(
    preferences: super::diarization_preferences::DiarizationPreferences,
) -> DiarizationPreferenceView {
    DiarizationPreferenceView {
        preferences,
        bounds: super::diarization_preferences::DiarizationPreferenceBounds::current(),
        is_default: preferences.is_default(),
    }
}

/// Read the separation tunables.
#[tauri::command]
pub async fn get_diarization_preferences<R: Runtime>(
    app: AppHandle<R>,
) -> Result<DiarizationPreferenceView, String> {
    Ok(preference_view(
        super::diarization_preferences::load_diarization_preferences(&app),
    ))
}

/// Write the separation tunables, clamped into their usable bands.
///
/// Returns what was stored rather than what was sent, so the interface shows the value that
/// will actually be used.
#[tauri::command]
pub async fn set_diarization_preferences<R: Runtime>(
    app: AppHandle<R>,
    preferences: super::diarization_preferences::DiarizationPreferences,
) -> Result<DiarizationPreferenceView, String> {
    let stored = super::diarization_preferences::save_diarization_preferences(&app, preferences)?;
    Ok(preference_view(stored))
}

/// Restore the calibrated defaults.
#[tauri::command]
pub async fn reset_diarization_preferences<R: Runtime>(
    app: AppHandle<R>,
) -> Result<DiarizationPreferenceView, String> {
    let defaults = super::diarization_preferences::DiarizationPreferences::default();
    let stored = super::diarization_preferences::save_diarization_preferences(&app, defaults)?;
    Ok(preference_view(stored))
}
#[cfg(test)]
mod tests {

    // -----------------------------------------------------------------------
    // The database path the command uses
    // -----------------------------------------------------------------------

    /// Build an in-memory database with the transcript table the command writes to.
    async fn test_pool() -> sqlx::SqlitePool {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .expect("in-memory database");

        sqlx::query(
            "CREATE TABLE transcripts (
                id TEXT PRIMARY KEY,
                meeting_id TEXT NOT NULL,
                transcript TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                summary TEXT,
                action_items TEXT,
                key_points TEXT,
                audio_start_time REAL,
                audio_end_time REAL,
                duration REAL,
                speaker TEXT
            )",
        )
        .execute(&pool)
        .await
        .expect("create table");

        pool
    }

    async fn insert_segment(
        pool: &sqlx::SqlitePool,
        id: &str,
        start: Option<f64>,
        end: Option<f64>,
    ) {
        sqlx::query(
            "INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, duration)
             VALUES (?, 'm1', 'text', '2026-01-01T00:00:00Z', ?, ?, 1.0)",
        )
        .bind(id)
        .bind(start)
        .bind(end)
        .execute(pool)
        .await
        .expect("insert");
    }

    #[tokio::test]
    async fn speaker_labels_are_written_and_readable() {
        // The whole point of the command: a label written here must come back out through
        // the `Transcript` model, or separation appears to do nothing.
        let pool = test_pool().await;
        insert_segment(&pool, "a", Some(0.0), Some(2.0)).await;
        insert_segment(&pool, "b", Some(2.0), Some(4.0)).await;

        for (id, label) in [("a", "speaker_1"), ("b", "speaker_2")] {
            sqlx::query("UPDATE transcripts SET speaker = ? WHERE id = ?")
                .bind(label)
                .bind(id)
                .execute(&pool)
                .await
                .expect("update speaker");
        }

        let rows: Vec<crate::database::models::Transcript> =
            sqlx::query_as("SELECT * FROM transcripts ORDER BY id")
                .fetch_all(&pool)
                .await
                .expect("read back");

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].speaker.as_deref(), Some("speaker_1"));
        assert_eq!(rows[1].speaker.as_deref(), Some("speaker_2"));
    }

    #[tokio::test]
    async fn the_transcript_model_carries_the_speaker_column() {
        // A regression guard: the column existed in the table long before the model had a
        // field for it, and `SELECT *` silently dropped it in between.
        let pool = test_pool().await;
        insert_segment(&pool, "a", Some(0.0), Some(1.0)).await;

        sqlx::query("UPDATE transcripts SET speaker = 'speaker_3' WHERE id = 'a'")
            .execute(&pool)
            .await
            .expect("update");

        let row: crate::database::models::Transcript =
            sqlx::query_as("SELECT * FROM transcripts WHERE id = 'a'")
                .fetch_one(&pool)
                .await
                .expect("reread");

        assert_eq!(
            row.speaker.as_deref(),
            Some("speaker_3"),
            "the speaker column must reach the model, not just the table"
        );
    }

    #[tokio::test]
    async fn segments_without_timing_are_not_selected_for_labelling() {
        // A segment with no timing cannot be placed on the timeline. It must be excluded
        // rather than defaulted to the first speaker.
        let pool = test_pool().await;
        insert_segment(&pool, "timed", Some(0.0), Some(1.0)).await;
        insert_segment(&pool, "untimed", None, None).await;

        let rows = load_transcript_rows(&pool, "m1").await.expect("load");
        let placeable: Vec<&TranscriptRow> = rows
            .iter()
            .filter(|row| row.audio_start_time.is_some() && row.audio_end_time.is_some())
            .collect();

        assert_eq!(rows.len(), 2, "both rows are read");
        assert_eq!(placeable.len(), 1, "only the timed row can be labelled");
        assert_eq!(placeable[0].id, "timed");
    }

    #[tokio::test]
    async fn loading_rows_returns_them_in_timeline_order() {
        // Attribution pairs rows with spans positionally, so the read order must be the
        // timeline order rather than whatever the table happens to return.
        let pool = test_pool().await;
        insert_segment(&pool, "late", Some(10.0), Some(11.0)).await;
        insert_segment(&pool, "early", Some(1.0), Some(2.0)).await;
        insert_segment(&pool, "middle", Some(5.0), Some(6.0)).await;

        let rows = load_transcript_rows(&pool, "m1").await.expect("load");
        let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();

        assert_eq!(ids, vec!["early", "middle", "late"]);
    }

    #[tokio::test]
    async fn loading_rows_for_another_meeting_returns_nothing() {
        let pool = test_pool().await;
        insert_segment(&pool, "a", Some(0.0), Some(1.0)).await;

        let rows = load_transcript_rows(&pool, "other").await.expect("load");
        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn writing_labels_does_not_touch_the_transcript_text() {
        // Separation must be re-runnable without disturbing the transcript itself.
        let pool = test_pool().await;
        insert_segment(&pool, "a", Some(0.0), Some(1.0)).await;

        sqlx::query("UPDATE transcripts SET speaker = 'speaker_1' WHERE id = 'a'")
            .execute(&pool)
            .await
            .expect("update");

        let row: crate::database::models::Transcript =
            sqlx::query_as("SELECT * FROM transcripts WHERE id = 'a'")
                .fetch_one(&pool)
                .await
                .expect("reread");

        assert_eq!(row.transcript, "text");
        assert_eq!(row.audio_start_time, Some(0.0));
        assert_eq!(row.audio_end_time, Some(1.0));
    }

    #[tokio::test]
    async fn relabelling_replaces_the_previous_label() {
        // Re-running separation must overwrite, not accumulate.
        let pool = test_pool().await;
        insert_segment(&pool, "a", Some(0.0), Some(1.0)).await;

        for label in ["speaker_1", "speaker_2"] {
            sqlx::query("UPDATE transcripts SET speaker = ? WHERE id = 'a'")
                .bind(label)
                .execute(&pool)
                .await
                .expect("update");
        }

        let row: crate::database::models::Transcript =
            sqlx::query_as("SELECT * FROM transcripts WHERE id = 'a'")
                .fetch_one(&pool)
                .await
                .expect("reread");

        assert_eq!(row.speaker.as_deref(), Some("speaker_2"));
    }
    use super::*;

    fn temp_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn write_sized(path: &Path, size: u64) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create dir");
        }
        std::fs::write(path, vec![0u8; size as usize]).expect("write");
    }

    #[test]
    fn status_of_an_empty_folder_reports_both_models_missing() {
        let dir = temp_dir();
        let status = status_for(dir.path());

        assert!(!status.ready);
        assert_eq!(status.missing.len(), 2);
        assert_eq!(status.required_bytes, catalog::total_bytes());
    }

    #[test]
    fn status_of_a_complete_folder_is_ready_with_nothing_required() {
        let dir = temp_dir();
        for artifact in catalog::REQUIRED {
            write_sized(&catalog::artifact_path(dir.path(), artifact), artifact.exact_bytes);
        }

        let status = status_for(dir.path());
        assert!(status.ready);
        assert!(status.missing.is_empty());
        assert_eq!(status.required_bytes, 0, "nothing should be needed");
    }

    #[test]
    fn status_asks_only_for_what_is_missing() {
        let dir = temp_dir();
        write_sized(
            &catalog::segmentation_path(dir.path()),
            catalog::SEGMENTATION.exact_bytes,
        );

        let status = status_for(dir.path());
        assert!(!status.ready);
        assert_eq!(status.missing, vec![catalog::EMBEDDING_FILENAME.to_string()]);
        assert_eq!(status.required_bytes, catalog::EMBEDDING.exact_bytes);
    }

    #[test]
    fn an_incomplete_file_is_reported_as_missing_so_it_is_re_downloaded() {
        let dir = temp_dir();
        write_sized(&catalog::segmentation_path(dir.path()), 1_000);

        let status = status_for(dir.path());
        assert!(status.missing.contains(&catalog::SEGMENTATION_FILENAME.to_string()));
    }

    #[test]
    fn the_folder_is_reported_so_the_ui_can_reveal_it() {
        let dir = temp_dir();
        let status = status_for(dir.path());
        assert_eq!(status.folder, dir.path().display().to_string());
    }

    #[test]
    fn the_partial_suffix_is_not_a_model_filename() {
        // A `.partial` file must never be picked up as a usable model.
        for artifact in catalog::REQUIRED {
            assert!(!artifact.filename.ends_with(PARTIAL_SUFFIX));
        }
    }

    // -----------------------------------------------------------------------
    // apply_speaker_labels: turning turns into stored labels
    // -----------------------------------------------------------------------

    fn row(id: &str, start: Option<f64>, end: Option<f64>) -> TranscriptRow {
        TranscriptRow {
            id: id.to_string(),
            audio_start_time: start,
            audio_end_time: end,
        }
    }

    fn turn(start: f64, end: f64, speaker: u32) -> crate::audio::speaker_turns::SpeakerTurn {
        crate::audio::speaker_turns::SpeakerTurn::new(start, end, speaker)
    }

    async fn speakers_of(pool: &sqlx::SqlitePool) -> Vec<(String, Option<String>)> {
        let rows: Vec<crate::database::models::Transcript> =
            sqlx::query_as("SELECT * FROM transcripts ORDER BY id")
                .fetch_all(pool)
                .await
                .expect("read back");
        rows.into_iter().map(|r| (r.id, r.speaker)).collect()
    }

    #[tokio::test]
    async fn apply_labels_writes_a_speaker_per_segment() {
        // The end-to-end shape of the command's write half: two turns covering two
        // segments, each labelled with the speaker it overlaps.
        let pool = test_pool().await;
        insert_segment(&pool, "a", Some(0.0), Some(2.0)).await;
        insert_segment(&pool, "b", Some(2.0), Some(4.0)).await;

        let rows = vec![
            row("a", Some(0.0), Some(2.0)),
            row("b", Some(2.0), Some(4.0)),
        ];
        let turns = vec![turn(0.0, 2.0, 0), turn(2.0, 4.0, 1)];

        let labelled = apply_speaker_labels(&pool, &rows, &turns)
            .await
            .expect("apply").labelled;

        assert_eq!(labelled, 2);
        assert_eq!(
            speakers_of(&pool).await,
            vec![
                ("a".to_string(), Some("speaker_1".to_string())),
                ("b".to_string(), Some("speaker_2".to_string())),
            ]
        );
    }

    #[tokio::test]
    async fn labels_are_one_based_as_stored() {
        // A zero-based index leaking into the label would read as "speaker 0".
        let pool = test_pool().await;
        insert_segment(&pool, "a", Some(0.0), Some(1.0)).await;

        apply_speaker_labels(&pool, &[row("a", Some(0.0), Some(1.0))], &[turn(0.0, 1.0, 0)])
            .await
            .expect("apply");

        assert_eq!(
            speakers_of(&pool).await[0].1.as_deref(),
            Some("speaker_1"),
            "the first speaker must be speaker_1, not speaker_0"
        );
    }

    #[tokio::test]
    async fn a_segment_straddling_two_turns_takes_the_larger_overlap() {
        // One person finishing while the next begins: the longer half decides.
        let pool = test_pool().await;
        insert_segment(&pool, "a", Some(3.0), Some(7.0)).await;

        // 2s of speaker 0, 2s of speaker 1 -> tie, resolved to the lower index.
        apply_speaker_labels(
            &pool,
            &[row("a", Some(3.0), Some(7.0))],
            &[turn(0.0, 5.0, 0), turn(5.0, 10.0, 1)],
        )
        .await
        .expect("apply");

        let stored = speakers_of(&pool).await;
        assert_eq!(stored[0].1.as_deref(), Some("speaker_1"), "tie goes to the lower index");

        // Now clearly mostly the second speaker.
        let pool2 = test_pool().await;
        insert_segment(&pool2, "a", Some(4.0), Some(9.0)).await;
        apply_speaker_labels(
            &pool2,
            &[row("a", Some(4.0), Some(9.0))],
            &[turn(0.0, 5.0, 0), turn(5.0, 10.0, 1)],
        )
        .await
        .expect("apply");

        assert_eq!(speakers_of(&pool2).await[0].1.as_deref(), Some("speaker_2"));
    }

    #[tokio::test]
    async fn rows_without_timing_are_left_unlabelled() {
        // Not defaulted to a speaker: a segment that cannot be placed on the timeline
        // must stay unlabelled rather than be attributed to whoever happens to be first.
        let pool = test_pool().await;
        insert_segment(&pool, "timed", Some(0.0), Some(1.0)).await;
        insert_segment(&pool, "untimed", None, None).await;

        let rows = vec![
            row("timed", Some(0.0), Some(1.0)),
            row("untimed", None, None),
        ];
        let labelled = apply_speaker_labels(&pool, &rows, &[turn(0.0, 1.0, 0)])
            .await
            .expect("apply").labelled;

        assert_eq!(labelled, 1, "only the timed row is labelled");
        assert_eq!(
            speakers_of(&pool).await,
            vec![
                ("timed".to_string(), Some("speaker_1".to_string())),
                ("untimed".to_string(), None),
            ]
        );
    }

    #[tokio::test]
    async fn no_turns_means_nothing_is_written() {
        // A recording with nothing separable must not have its segments relabelled.
        let pool = test_pool().await;
        insert_segment(&pool, "a", Some(0.0), Some(1.0)).await;

        let labelled = apply_speaker_labels(&pool, &[row("a", Some(0.0), Some(1.0))], &[])
            .await
            .expect("apply").labelled;

        assert_eq!(labelled, 0);
        assert_eq!(speakers_of(&pool).await[0].1, None);
    }

    #[tokio::test]
    async fn only_the_given_rows_are_touched() {
        // Another meeting's segments must be untouched even if their timings overlap.
        let pool = test_pool().await;
        insert_segment(&pool, "a", Some(0.0), Some(1.0)).await;
        insert_segment(&pool, "b", Some(0.0), Some(1.0)).await;

        apply_speaker_labels(&pool, &[row("a", Some(0.0), Some(1.0))], &[turn(0.0, 1.0, 0)])
            .await
            .expect("apply");

        let stored = speakers_of(&pool).await;
        assert_eq!(stored[0].1.as_deref(), Some("speaker_1"));
        assert_eq!(stored[1].1, None, "row b was not in the request and must not change");
    }

    #[tokio::test]
    async fn relabelling_overwrites_rather_than_accumulates() {
        let pool = test_pool().await;
        insert_segment(&pool, "a", Some(0.0), Some(1.0)).await;

        apply_speaker_labels(&pool, &[row("a", Some(0.0), Some(1.0))], &[turn(0.0, 1.0, 0)])
            .await
            .expect("first");
        apply_speaker_labels(&pool, &[row("a", Some(0.0), Some(1.0))], &[turn(0.0, 1.0, 4)])
            .await
            .expect("second");

        assert_eq!(speakers_of(&pool).await[0].1.as_deref(), Some("speaker_5"));
    }

    #[tokio::test]
    async fn a_segment_in_a_gap_still_gets_the_nearest_speaker() {
        // Leaving it unlabelled would punch a hole in the conversation.
        let pool = test_pool().await;
        insert_segment(&pool, "a", Some(4.2), Some(4.8)).await;

        apply_speaker_labels(
            &pool,
            &[row("a", Some(4.2), Some(4.8))],
            &[turn(0.0, 4.0, 0), turn(6.0, 10.0, 1)],
        )
        .await
        .expect("apply");

        assert_eq!(speakers_of(&pool).await[0].1.as_deref(), Some("speaker_1"));
    }

    #[tokio::test]
    async fn the_transcript_text_survives_labelling() {
        // Separation annotates; it must not rewrite what was said.
        let pool = test_pool().await;
        insert_segment(&pool, "a", Some(0.0), Some(1.0)).await;

        apply_speaker_labels(&pool, &[row("a", Some(0.0), Some(1.0))], &[turn(0.0, 1.0, 0)])
            .await
            .expect("apply");

        let stored: crate::database::models::Transcript =
            sqlx::query_as("SELECT * FROM transcripts WHERE id = 'a'")
                .fetch_one(&pool)
                .await
                .expect("read");

        assert_eq!(stored.transcript, "text");
        assert_eq!(stored.audio_start_time, Some(0.0));
        assert_eq!(stored.audio_end_time, Some(1.0));
    }
    // -----------------------------------------------------------------------
    // The download path
    // -----------------------------------------------------------------------

    /// The models are fetched, not bundled, so this is the only path that makes the feature
    /// usable on a fresh install — and until now nothing exercised it.
    ///
    /// Downloads the smaller of the two artifacts over the real network, through the same
    /// proxy-aware client the command uses.
    #[tokio::test]
    #[ignore = "downloads ~6 MB over the network; run with --ignored --nocapture"]
    async fn the_segmentation_model_downloads_to_its_final_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let final_path = catalog::artifact_path(dir.path(), &catalog::SEGMENTATION);
        let partial_path =
            dir.path().join(format!("{}{PARTIAL_SUFFIX}", catalog::SEGMENTATION.filename));

        // Nothing is present to begin with, so a passing test cannot be an accident of a
        // file that was already there.
        assert!(!final_path.exists(), "the fixture directory must start empty");
        assert!(
            catalog::missing_artifacts(dir.path())
                .iter()
                .any(|artifact| artifact.filename == catalog::SEGMENTATION.filename),
            "the artifact must be reported missing before the download"
        );

        let client = crate::http_client::client_with_proxy();
        download_one(&client, dir.path(), &catalog::SEGMENTATION)
            .await
            .expect("the download must succeed");

        // The file must be at the final path, at exactly the expected size. A truncated
        // body would otherwise be stored and only fail later, inside ONNX.
        assert!(
            final_path.is_file(),
            "the model must land at {}",
            final_path.display()
        );
        let written = std::fs::metadata(&final_path).expect("metadata").len();
        assert_eq!(
            written,
            catalog::SEGMENTATION.exact_bytes,
            "the downloaded file is the wrong size"
        );

        // A leftover partial would be picked up as usable by anything listing the folder.
        assert!(
            !partial_path.exists(),
            "a .partial file was left behind at {}",
            partial_path.display()
        );

        // The catalog must now consider this artifact present…
        assert!(
            !catalog::missing_artifacts(dir.path())
                .iter()
                .any(|artifact| artifact.filename == catalog::SEGMENTATION.filename),
            "the artifact must no longer be reported missing"
        );

        // …while the set as a whole is still incomplete, because the embedding model is
        // absent. Reporting ready here would let a run start and fail on the second model.
        assert!(
            !catalog::is_ready(dir.path()),
            "one of two artifacts is not a usable set"
        );
    }

    /// Re-downloading replaces the file rather than failing on the one already there.
    #[tokio::test]
    #[ignore = "downloads ~6 MB over the network; run with --ignored --nocapture"]
    async fn a_second_download_replaces_a_corrupt_file() {
        // Offered in the interface as "download again", which is the remedy for a truncated
        // file, so it has to work when the destination already exists.
        let dir = tempfile::tempdir().expect("tempdir");
        let final_path = catalog::artifact_path(dir.path(), &catalog::SEGMENTATION);

        std::fs::write(&final_path, b"not a model").expect("write a corrupt file");
        let corrupt_size = std::fs::metadata(&final_path).expect("metadata").len();
        assert!(corrupt_size < catalog::SEGMENTATION.exact_bytes);

        let client = crate::http_client::client_with_proxy();
        download_one(&client, dir.path(), &catalog::SEGMENTATION)
            .await
            .expect("re-downloading over an existing file must succeed");

        assert_eq!(
            std::fs::metadata(&final_path).expect("metadata").len(),
            catalog::SEGMENTATION.exact_bytes,
            "the corrupt file must have been replaced, not left in place"
        );

        let partial_path =
            dir.path().join(format!("{}{PARTIAL_SUFFIX}", catalog::SEGMENTATION.filename));
        assert!(!partial_path.exists(), "a .partial file was left behind");
    }}
