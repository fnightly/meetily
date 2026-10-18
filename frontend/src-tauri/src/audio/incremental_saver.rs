use std::path::PathBuf;
use anyhow::{Result, anyhow};
use log::{info, warn, error};
use super::encode::{encode_single_audio, transcode_audio};
use super::formats::AudioEncodingSettings;
use super::recording_state::AudioChunk;
use serde::{Serialize, Deserialize};

use super::ffmpeg::find_ffmpeg_path;

/// Audio data without device type (we only store mixed audio)
#[derive(Clone)]
struct AudioData {
    data: Vec<f32>,
    // sample_rate: u32,
}

/// Length of one checkpoint, in seconds of frames.
const CHECKPOINT_SECONDS: u32 = 30;

/// Interleaved sample count that makes up `seconds` of audio.
///
/// A stereo chunk interleaves two samples per frame, so the same duration holds
/// twice as many samples as mono. Comparing a mono-derived threshold against
/// stereo data would write checkpoints at half the intended duration.
fn samples_for_seconds(seconds: u32, sample_rate: u32, channels: u16) -> usize {
    sample_rate as usize * channels.max(1) as usize * seconds as usize
}

/// Duration in seconds of `interleaved_samples` at `sample_rate`/`channels`.
///
/// Duration counts frames, not samples: 48 kHz stereo data holds two samples per
/// frame, so dividing by the sample rate alone would report double the length.
fn duration_of_samples(interleaved_samples: usize, sample_rate: u32, channels: u16) -> f64 {
    let samples_per_second = sample_rate as f64 * channels.max(1) as f64;

    if samples_per_second <= 0.0 {
        0.0
    } else {
        interleaved_samples as f64 / samples_per_second
    }
}

/// Incremental audio saver that writes checkpoints every 30 seconds
/// to minimize memory usage and enable crash recovery
pub struct IncrementalAudioSaver {
    checkpoint_buffer: Vec<AudioData>,
    /// Interleaved sample count of one checkpoint (30 s of frames).
    checkpoint_interval_samples: usize,
    checkpoint_count: u32,
    checkpoints_dir: PathBuf,
    meeting_folder: PathBuf,
    sample_rate: u32,
    /// Channel count every incoming chunk's samples are interpreted with.
    ///
    /// `AudioChunk` carries no channel information, so this is the session-level
    /// fact that says whether the interleaved samples are mono or stereo.
    channels: u16,
    /// Format/quality the finished recording should be written as. Checkpoints
    /// stay AAC/MP4 regardless; see `finalize`.
    encoding: AudioEncodingSettings,
}

impl IncrementalAudioSaver {
    /// Create a new incremental saver
    ///
    /// # Arguments
    /// * `meeting_folder` - Path to the meeting folder (contains .checkpoints/)
    /// * `sample_rate` - Sample rate of audio (typically 48000)
    pub fn new(meeting_folder: PathBuf, sample_rate: u32) -> Result<Self> {
        // Mono sessions are the historical default; the recording manager passes
        // the real channel count through `with_encoding`.
        Self::with_encoding(
            meeting_folder,
            sample_rate,
            1,
            AudioEncodingSettings::default(),
        )
    }

    /// Create a saver that writes `channels`-channel audio in `encoding`.
    pub fn with_encoding(
        meeting_folder: PathBuf,
        sample_rate: u32,
        channels: u16,
        encoding: AudioEncodingSettings,
    ) -> Result<Self> {
        let checkpoints_dir = meeting_folder.join(".checkpoints");

        // Verify checkpoints directory exists
        if !checkpoints_dir.exists() {
            return Err(anyhow!("Checkpoints directory does not exist: {}", checkpoints_dir.display()));
        }

        // A chunk without a channel count is mono; zero would make every frame
        // count and duration come out as zero/infinity.
        let channels = channels.max(1);

        Ok(Self {
            checkpoint_buffer: Vec::new(),
            checkpoint_interval_samples: samples_for_seconds(CHECKPOINT_SECONDS, sample_rate, channels),
            checkpoint_count: 0,
            checkpoints_dir,
            meeting_folder,
            sample_rate,
            channels,
            encoding,
        })
    }

    /// Channel count this session's chunks are interpreted with.
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// Add an audio chunk to the buffer
    /// Automatically saves a checkpoint when buffer reaches 30 seconds
    pub fn add_chunk(&mut self, chunk: AudioChunk) -> Result<()> {
        let audio_data = AudioData {
            data: chunk.data,
            // sample_rate: chunk.sample_rate,
        };

        self.checkpoint_buffer.push(audio_data);

        // Total interleaved samples in the buffer. The threshold already includes
        // the channel count, so this still means 30 seconds of frames.
        let total_samples: usize = self.checkpoint_buffer
            .iter()
            .map(|c| c.data.len())
            .sum();

        // Save checkpoint when buffer reaches threshold (30 seconds)
        if total_samples >= self.checkpoint_interval_samples {
            self.save_checkpoint()?;
            self.checkpoint_buffer.clear();
        }

        Ok(())
    }

    /// Save current buffer as a checkpoint file
    fn save_checkpoint(&mut self) -> Result<()> {
        // Concatenate all chunks in buffer
        let audio_data: Vec<f32> = self.checkpoint_buffer
            .iter()
            .flat_map(|c| &c.data)
            .cloned()
            .collect();

        if audio_data.is_empty() {
            warn!("Attempted to save empty checkpoint, skipping");
            return Ok(());
        }

        // Generate checkpoint filename
        let checkpoint_path = self.checkpoints_dir
            .join(format!("audio_chunk_{:03}.mp4", self.checkpoint_count));

        let channels = self.channels;

        // Encode and save checkpoint with the session's channel layout and a
        // channel-aware bitrate; a hardcoded mono here would fold stereo down.
        encode_single_audio(
            bytemuck::cast_slice(&audio_data),
            self.sample_rate,
            channels,
            self.encoding.lossy_bitrate_kbps(channels),
            &checkpoint_path
        )?;

        // Duration counts frames: interleaved stereo holds two samples per frame.
        let duration_seconds = duration_of_samples(audio_data.len(), self.sample_rate, channels);
        let frame_count = audio_data.len() / channels as usize;
        self.checkpoint_count += 1;

        info!("Saved checkpoint {}: {:.2}s of audio ({} frames, {} interleaved samples)",
              self.checkpoint_count,
              duration_seconds,
              frame_count,
              audio_data.len());

        Ok(())
    }

    /// Finalize the recording: save final checkpoint, merge all checkpoints,
    /// transcode into the requested format, then clean up.
    ///
    /// Returns the path to the finished recording (extension depends on the
    /// configured format).
    pub async fn finalize(&mut self) -> Result<PathBuf> {
        info!(
            "Finalizing incremental recording (target format: {}, {} channels)...",
            self.encoding.format.id(),
            self.channels
        );

        // Save final buffer if not empty
        if !self.checkpoint_buffer.is_empty() {
            info!("Saving final checkpoint with remaining {} chunks", self.checkpoint_buffer.len());
            self.save_checkpoint()?;
            self.checkpoint_buffer.clear();
        }

        if self.checkpoint_count == 0 {
            return Err(anyhow!("No audio checkpoints to merge - recording may have failed"));
        }

        let final_audio_path = self.meeting_folder.join(self.encoding.output_file_name());

        // Checkpoints are AAC in MP4. Concatenating them stays cheap and keeps the
        // crash-recovery path untouched, so the user's format is produced by a
        // single pass over the merged file rather than by encoding every 30-second
        // chunk in a format that may not concatenate cleanly.
        //
        // The final pass runs even when the container already matches, because it
        // is also what writes the metadata tags. For a matching container it is a
        // stream copy, so it costs no re-encoding.
        let intermediate = self.meeting_folder.join(".merged-intermediate.mp4");
        self.merge_checkpoints(&intermediate).await?;

        // Re-encoding happens only when the target container differs from AAC/MP4.
        let mut settings = self.encoding.clone();
        if matches!(settings.format, super::formats::RecordingFormat::M4a) {
            settings.copy_stream = true;
        }

        let merged_for_final = intermediate.clone();
        let target = final_audio_path.clone();
        let target_label = settings.format.label();
        let target_id = settings.format.id().to_string();
        // Passed through so PCM keeps the stereo layout and lossy targets get
        // the channel-aware bitrate.
        let channels = self.channels;

        // FFmpeg is CPU-bound; keep it off the async runtime so it cannot stall
        // other tasks during teardown.
        let finalize_result = tokio::task::spawn_blocking(move || {
            transcode_audio(&merged_for_final, &target, settings, channels)
        })
        .await
        .map_err(|e| anyhow!("Finalize task failed to join: {}", e))?;

        match finalize_result {
            Ok(()) => {
                if let Err(e) = std::fs::remove_file(&intermediate) {
                    warn!("Failed to remove finalize intermediate: {}", e);
                }
            }
            Err(e) => {
                // Keep the audio rather than lose the recording: fall back to the
                // merged AAC file and report where it really is.
                error!("Producing {} failed: {}", target_id, e);
                warn!(
                    "Keeping the merged AAC file as {} instead",
                    intermediate.display()
                );
                return Err(anyhow!(
                    "Recording saved as AAC ({}), but producing {} failed: {}. \
                     The audio is intact at {}.",
                    intermediate.display(),
                    target_label,
                    e,
                    intermediate.display()
                ));
            }
        }

        // Clean up checkpoints directory
        info!("Cleaning up {} checkpoint files", self.checkpoint_count);
        if let Err(e) = std::fs::remove_dir_all(&self.checkpoints_dir) {
            warn!("Failed to clean up checkpoints directory: {}", e);
            // Non-fatal - user can manually delete
        }

        info!("Finalized recording: {}", final_audio_path.display());

        Ok(final_audio_path)
    }

    /// Merge all checkpoint files into final audio.mp4 using FFmpeg concat
    /// Uses concat demuxer for fast merging without re-encoding
    async fn merge_checkpoints(&self, output: &PathBuf) -> Result<()> {
        info!("Merging {} checkpoints into final audio file...", self.checkpoint_count);

        // Create concat list file for FFmpeg
        let list_file = self.checkpoints_dir.join("concat_list.txt");
        let mut list_content = String::new();

        for i in 0..self.checkpoint_count {
            let checkpoint_path = self.checkpoints_dir
                .join(format!("audio_chunk_{:03}.mp4", i));

            // Verify checkpoint exists
            if !checkpoint_path.exists() {
                return Err(anyhow!("Checkpoint file missing: {}", checkpoint_path.display()));
            }

            // Use absolute path for FFmpeg (required for safe mode)
            let abs_path = checkpoint_path.canonicalize()?;
            list_content.push_str(&format!("file '{}'\n", abs_path.display()));
        }

        std::fs::write(&list_file, list_content)?;

        let ffmpeg_path = find_ffmpeg_path()
            .ok_or_else(|| anyhow!("FFmpeg not found. Please install FFmpeg to finalize recordings."))?;
        info!("Using FFmpeg at: {:?}", ffmpeg_path);

        // Run FFmpeg concat command
        // Using concat demuxer with copy codec for fast merging (no re-encoding)
        
        let mut command = std::process::Command::new(ffmpeg_path);
        
        command.args(&[
            "-f", "concat",          // Use concat demuxer
            "-safe", "0",            // Allow absolute paths
            "-i", list_file.to_str().unwrap(),
            "-c", "copy",            // Copy codec - no re-encoding!
            "-y",                    // Overwrite output file
            output.to_str().unwrap()
        ]);

        // Hide console window on Windows to prevent CMD popup during finalization
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let ffmpeg_output = command.output()?;

        if !ffmpeg_output.status.success() {
            let stderr = String::from_utf8_lossy(&ffmpeg_output.stderr);
            error!("FFmpeg merge failed: {}", stderr);
            return Err(anyhow!("FFmpeg concat failed: {}", stderr));
        }

        // Verify output file was created
        if !output.exists() {
            return Err(anyhow!("Merged audio file was not created: {}", output.display()));
        }

        info!("Successfully merged {} checkpoints → {}",
              self.checkpoint_count, output.display());

        Ok(())
    }

    /// Get the meeting folder path
    pub fn get_meeting_folder(&self) -> &PathBuf {
        &self.meeting_folder
    }

    /// Get current checkpoint count
    pub fn get_checkpoint_count(&self) -> u32 {
        self.checkpoint_count
    }
}

/// Audio recovery status for transcript recovery feature
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioRecoveryStatus {
    pub status: String, // "success" | "partial" | "failed" | "none"
    pub chunk_count: u32,
    pub estimated_duration_seconds: f64,
    pub audio_file_path: Option<String>,
    pub message: String,
}

/// Recover audio from checkpoint files
/// This is called by the transcript recovery system to merge audio chunks after a crash
#[tauri::command]
pub async fn recover_audio_from_checkpoints(
    meeting_folder: String,
    _sample_rate: u32
) -> Result<AudioRecoveryStatus, String> {
    info!("Starting audio recovery for folder: {}", meeting_folder);

    let folder_path = PathBuf::from(&meeting_folder);
    let checkpoints_dir = folder_path.join(".checkpoints");

    // Check if checkpoints directory exists
    if !checkpoints_dir.exists() {
        info!("No checkpoints directory found at: {}", checkpoints_dir.display());
        return Ok(AudioRecoveryStatus {
            status: "none".to_string(),
            chunk_count: 0,
            estimated_duration_seconds: 0.0,
            audio_file_path: None,
            message: "No audio checkpoints found".to_string(),
        });
    }

    // Scan for checkpoint files
    let mut checkpoint_files: Vec<_> = std::fs::read_dir(&checkpoints_dir)
        .map_err(|e| format!("Failed to read checkpoints directory: {}", e))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.path().extension().and_then(|s| s.to_str()) == Some("mp4")
        })
        .collect();

    if checkpoint_files.is_empty() {
        info!("No checkpoint files found in: {}", checkpoints_dir.display());
        return Ok(AudioRecoveryStatus {
            status: "none".to_string(),
            chunk_count: 0,
            estimated_duration_seconds: 0.0,
            audio_file_path: None,
            message: "No audio checkpoint files found".to_string(),
        });
    }

    // Sort by filename (audio_chunk_000.mp4, audio_chunk_001.mp4, etc.)
    checkpoint_files.sort_by_key(|entry| entry.path());

    let chunk_count = checkpoint_files.len() as u32;
    let estimated_duration = (chunk_count as f64) * 30.0; // 30 seconds per chunk

    info!("Found {} checkpoint files, estimated duration: {:.2}s", chunk_count, estimated_duration);

    // Create FFmpeg concat file
    let concat_file_path = checkpoints_dir.join("concat_list.txt");
    let mut concat_content = String::new();

    for entry in &checkpoint_files {
        let path = entry.path().canonicalize()
            .map_err(|e| format!("Failed to canonicalize path: {}", e))?;
        concat_content.push_str(&format!("file '{}'\n", path.display()));
    }

    std::fs::write(&concat_file_path, concat_content)
        .map_err(|e| format!("Failed to write concat file: {}", e))?;

    // Run FFmpeg to merge chunks
    let output_path = folder_path.join("audio.mp4");
    let output_path_str = output_path.to_str()
        .ok_or("Invalid output path")?
        .to_string();

    let ffmpeg_path = find_ffmpeg_path()
        .ok_or_else(|| "FFmpeg not found. Please install FFmpeg to recover audio.".to_string())?;
    info!("Using FFmpeg at: {:?}", ffmpeg_path);

    let mut command = std::process::Command::new(ffmpeg_path);

    command.args(&[
        "-f", "concat",
        "-safe", "0",
        "-i", concat_file_path.to_str().unwrap(),
        "-c", "copy",
        "-y", // Overwrite if exists
        &output_path_str
    ]);

    // Hide console window on Windows
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let ffmpeg_result = command.output();

    match ffmpeg_result {
        Ok(output) if output.status.success() => {
            // Clean up concat file
            let _ = std::fs::remove_file(concat_file_path);

            info!("Successfully recovered audio: {}", output_path_str);

            Ok(AudioRecoveryStatus {
                status: "success".to_string(),
                chunk_count,
                estimated_duration_seconds: estimated_duration,
                audio_file_path: Some(output_path_str),
                message: format!("Successfully recovered {} audio chunks", chunk_count),
            })
        }
        Ok(output) => {
            let error = String::from_utf8_lossy(&output.stderr);
            error!("FFmpeg recovery failed: {}", error);
            Ok(AudioRecoveryStatus {
                status: "failed".to_string(),
                chunk_count,
                estimated_duration_seconds: estimated_duration,
                audio_file_path: None,
                message: format!("FFmpeg failed: {}", error),
            })
        }
        Err(e) => {
            error!("Failed to run FFmpeg: {}", e);
            Ok(AudioRecoveryStatus {
                status: "failed".to_string(),
                chunk_count,
                estimated_duration_seconds: estimated_duration,
                audio_file_path: None,
                message: format!("Failed to run FFmpeg: {}", e),
            })
        }
    }
}

/// Clean up checkpoint files after successful recording or recovery
/// This command is called by the frontend after successful save to clean up checkpoint files
#[tauri::command]
pub async fn cleanup_checkpoints(meeting_folder: String) -> Result<(), String> {
    info!("Cleaning up checkpoints for folder: {}", meeting_folder);

    let folder_path = PathBuf::from(&meeting_folder);
    let checkpoints_dir = folder_path.join(".checkpoints");

    if checkpoints_dir.exists() {
        std::fs::remove_dir_all(&checkpoints_dir)
            .map_err(|e| format!("Failed to remove checkpoints directory: {}", e))?;
        info!("Successfully cleaned up checkpoints directory");
    } else {
        info!("No checkpoints directory to clean up");
    }

    Ok(())
}

/// Check if a meeting folder has audio checkpoint files
/// Returns true if .checkpoints/ directory exists and contains .mp4 files
#[tauri::command]
pub async fn has_audio_checkpoints(meeting_folder: String) -> Result<bool, String> {
    let folder_path = PathBuf::from(&meeting_folder);
    let checkpoints_dir = folder_path.join(".checkpoints");

    // Check if checkpoints directory exists
    if !checkpoints_dir.exists() {
        return Ok(false);
    }

    // Scan for .mp4 checkpoint files
    let has_mp4_files = std::fs::read_dir(&checkpoints_dir)
        .map_err(|e| format!("Failed to read checkpoints directory: {}", e))?
        .filter_map(|entry| entry.ok())
        .any(|entry| {
            entry.path().extension().and_then(|s| s.to_str()) == Some("mp4")
        });

    Ok(has_mp4_files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use super::super::recording_state::DeviceType;

    #[tokio::test]
    async fn test_checkpoint_creation() {
        // Create temp meeting folder
        let temp_dir = tempdir().unwrap();
        let meeting_folder = temp_dir.path().join("Test_Meeting");
        std::fs::create_dir_all(&meeting_folder).unwrap();
        std::fs::create_dir_all(meeting_folder.join(".checkpoints")).unwrap();

        let mut saver = IncrementalAudioSaver::new(
            meeting_folder.clone(),
            48000
        ).unwrap();

        // Add 60 seconds worth of audio (should create 2 checkpoints)
        for i in 0..120 {  // 120 chunks of 0.5s each
            let chunk = AudioChunk {
                data: vec![0.5f32; 24000],  // 0.5s at 48kHz
                sample_rate: 48000,
                timestamp: i as f64 * 0.5,  // timestamp in seconds
                chunk_id: i as u64,
                device_type: DeviceType::Microphone,
            };
            saver.add_chunk(chunk).unwrap();
        }

        // Verify 2 checkpoints created
        assert_eq!(saver.checkpoint_count, 2);

        // Finalize and verify merge
        let final_path = saver.finalize().await.unwrap();
        assert!(final_path.exists());

        // Verify checkpoints directory deleted
        assert!(!meeting_folder.join(".checkpoints").exists());
    }

    #[tokio::test]
    async fn test_empty_recording() {
        let temp_dir = tempdir().unwrap();
        let meeting_folder = temp_dir.path().join("Empty_Test");
        std::fs::create_dir_all(&meeting_folder).unwrap();
        std::fs::create_dir_all(meeting_folder.join(".checkpoints")).unwrap();

        let mut saver = IncrementalAudioSaver::new(
            meeting_folder.clone(),
            48000
        ).unwrap();

        // Try to finalize without adding any chunks
        let result = saver.finalize().await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No audio checkpoints"));
    }

    #[test]
    fn checkpoint_interval_counts_interleaved_samples() {
        // 30 s at 48 kHz: stereo interleaves two samples per frame, so it holds
        // twice as many samples as mono for the same duration.
        assert_eq!(samples_for_seconds(CHECKPOINT_SECONDS, 48000, 1), 1_440_000);
        assert_eq!(samples_for_seconds(CHECKPOINT_SECONDS, 48000, 2), 2_880_000);
        // A missing/zero channel count is treated as mono rather than zero.
        assert_eq!(samples_for_seconds(CHECKPOINT_SECONDS, 48000, 0), 1_440_000);
    }

    #[test]
    fn duration_is_measured_in_frames_not_samples() {
        assert!((duration_of_samples(1_440_000, 48_000, 1) - 30.0).abs() < f64::EPSILON);
        assert!((duration_of_samples(2_880_000, 48_000, 2) - 30.0).abs() < f64::EPSILON);
        // Interpreting stereo interleaving as mono would report double length.
        assert!((duration_of_samples(2_880_000, 48_000, 1) - 60.0).abs() < f64::EPSILON);
        assert_eq!(duration_of_samples(0, 0, 2), 0.0);
    }

    fn silent_chunk(samples: usize, index: u64) -> AudioChunk {
        AudioChunk {
            data: vec![0.0f32; samples],
            sample_rate: 48000,
            timestamp: index as f64 * 0.5,
            chunk_id: index,
            device_type: DeviceType::Microphone,
        }
    }

    #[test]
    fn a_stereo_saver_needs_thirty_seconds_of_stereo_frames() {
        // No FFmpeg: staying one sample below the threshold only exercises the
        // frame maths, and proves the threshold was not derived from mono.
        let temp_dir = tempdir().unwrap();
        let meeting_folder = temp_dir.path().join("Stereo_Test");
        std::fs::create_dir_all(&meeting_folder).unwrap();
        std::fs::create_dir_all(meeting_folder.join(".checkpoints")).unwrap();

        let mut saver = IncrementalAudioSaver::with_encoding(
            meeting_folder,
            48000,
            2,
            AudioEncodingSettings::default(),
        )
        .unwrap();

        assert_eq!(saver.channels(), 2);
        assert_eq!(saver.checkpoint_interval_samples, 2_880_000);

        // 2,879,999 interleaved samples: just under 30 s of stereo frames, but
        // above the mono threshold, so a mono threshold would have checkpointed.
        saver.add_chunk(silent_chunk(2_000_000, 0)).unwrap();
        saver.add_chunk(silent_chunk(879_999, 1)).unwrap();

        assert_eq!(saver.get_checkpoint_count(), 0);
    }

    #[test]
    fn a_mono_saver_keeps_the_mono_threshold() {
        let temp_dir = tempdir().unwrap();
        let meeting_folder = temp_dir.path().join("Mono_Test");
        std::fs::create_dir_all(&meeting_folder).unwrap();
        std::fs::create_dir_all(meeting_folder.join(".checkpoints")).unwrap();

        let saver = IncrementalAudioSaver::new(meeting_folder, 48000).unwrap();

        assert_eq!(saver.channels(), 1);
        assert_eq!(saver.checkpoint_interval_samples, 1_440_000);
    }

    /// Real-FFmpeg check that a stereo session survives checkpoint -> transcode.
    ///
    /// The channel-aware unit tests prove the arguments and the frame maths; this
    /// one proves the intermediate AAC checkpoints and the final transcode keep two
    /// genuinely different channels instead of silently folding to mono.
    #[tokio::test]
    async fn a_stereo_recording_stays_stereo_through_the_encoder() {
        use crate::audio::formats::{RecordingFormat, RecordingQuality};
        use crate::audio::formats::AudioEncodingSettings;

        let temp_dir = tempdir().unwrap();
        let meeting_folder = temp_dir.path().join("Stereo_Encode_Test");
        std::fs::create_dir_all(&meeting_folder).unwrap();
        std::fs::create_dir_all(meeting_folder.join(".checkpoints")).unwrap();

        let mut saver = IncrementalAudioSaver::with_encoding(
            meeting_folder,
            48000,
            2,
            AudioEncodingSettings::new(RecordingFormat::Mp3, RecordingQuality::High),
        )
        .unwrap();

        // Three seconds, different tone per channel, so a mono fold is detectable.
        const FRAMES_PER_CHUNK: usize = 24_000; // 0.5 s
        for chunk_index in 0..6u64 {
            let start_frame = chunk_index as usize * FRAMES_PER_CHUNK;
            let mut data = Vec::with_capacity(FRAMES_PER_CHUNK * 2);
            for frame in 0..FRAMES_PER_CHUNK {
                let t = (start_frame + frame) as f32 / 48_000.0;
                data.push(0.25 * (2.0 * std::f32::consts::PI * 440.0 * t).sin());
                data.push(0.25 * (2.0 * std::f32::consts::PI * 880.0 * t).sin());
            }
            saver
                .add_chunk(AudioChunk {
                    data,
                    sample_rate: 48000,
                    timestamp: chunk_index as f64 * 0.5,
                    chunk_id: chunk_index,
                    device_type: DeviceType::Microphone,
                })
                .unwrap();
        }

        let output = saver.finalize().await.expect("stereo finalize");
        assert!(output.exists(), "the finished recording was not written");

        let ffmpeg = crate::audio::ffmpeg::find_ffmpeg_path().expect("ffmpeg available for this test");
        let decoded = std::process::Command::new(&ffmpeg)
            .args([
                "-v", "error",
                "-i", output.to_str().unwrap(),
                "-f", "f32le",
                "-acodec", "pcm_f32le",
                "-",
            ])
            .output()
            .expect("run ffmpeg to inspect the recording");
        assert!(
            decoded.status.success(),
            "ffmpeg could not read the stereo recording: {}",
            String::from_utf8_lossy(&decoded.stderr)
        );

        let samples: Vec<f32> = decoded
            .stdout
            .chunks_exact(4)
            .map(|bytes| f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            .collect();
        assert!(
            samples.len() >= 2 * 48_000 * 2,
            "the stereo recording decoded to {} samples, expected at least two channels of two seconds",
            samples.len()
        );

        // The two channels carry different tones; if the encode folded to mono they
        // would be identical (or the decode would return one channel per frame).
        let differing = samples
            .chunks_exact(2)
            .take(48_000)
            .filter(|pair| (pair[0] - pair[1]).abs() > 0.05)
            .count();
        // Two sines cross and can coincide for short stretches, so require a clear
        // majority rather than every frame; a mono fold would leave almost none.
        assert!(
            differing > 15_000,
            "the two decoded channels are not independent ({differing} differing frames)"
        );
    }
}
