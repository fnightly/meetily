use super::ffmpeg::find_ffmpeg_path; // Correct path to encode module
use super::AudioDevice;
use std::io::Write;
use std::sync::Arc;
use std::{
    path::PathBuf,
    process::{Command, Stdio},
};
use tracing::{debug, error};

pub struct AudioInput {
    pub data: Arc<Vec<f32>>,
    pub sample_rate: u32,
    pub channels: u16,
    pub device: Arc<AudioDevice>,
}

pub fn encode_single_audio(
    data: &[u8],
    sample_rate: u32,
    channels: u16,
    output_path: &PathBuf,
) -> anyhow::Result<()> {
    debug!("Starting FFmpeg process for {} bytes of audio data", data.len());

    if data.is_empty() {
        return Err(anyhow::anyhow!("No audio data provided for encoding"));
    }

    let ffmpeg_path = find_ffmpeg_path().ok_or_else(|| {
        anyhow::anyhow!("FFmpeg not found. Please install FFmpeg to save recordings.")
    })?;

    debug!("Using FFmpeg at: {:?}", ffmpeg_path);

    let mut command = Command::new(ffmpeg_path);
    command
        .args([
            "-f",
            "f32le",
            "-ar",
            &sample_rate.to_string(),
            "-ac",
            &channels.to_string(),
            "-i",
            "pipe:0",
            "-c:a",
            "aac",
            "-b:a",
            "192k", // Increased from 64k for better audio quality (especially for speech)
            "-profile:a",
            "aac_low", // Use AAC-LC profile for better compatibility
            "-movflags",
            "+faststart", // Optimize for web streaming
            "-f",
            "mp4",
            output_path.to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Hide console window on Windows to prevent CMD popup during recording
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    debug!("FFmpeg command: {:?}", command);

    #[allow(clippy::zombie_processes)]
    let mut ffmpeg = command.spawn().expect("Failed to spawn FFmpeg process");
    debug!("FFmpeg process spawned");
    let mut stdin = ffmpeg.stdin.take().expect("Failed to open stdin");

    stdin.write_all(data)?;

    debug!("Dropping stdin");
    drop(stdin);
    debug!("Waiting for FFmpeg process to exit");
    let output = ffmpeg.wait_with_output().unwrap();
    let status = output.status;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    debug!("FFmpeg process exited with status: {}", status);
    debug!("FFmpeg stdout: {}", stdout);
    debug!("FFmpeg stderr: {}", stderr);

    if !status.success() {
        error!("FFmpeg process failed with status: {}", status);
        error!("FFmpeg stderr: {}", stderr);
        return Err(anyhow::anyhow!(
            "FFmpeg process failed with status: {}",
            status
        ));
    }

    Ok(())
}

/// Re-encode an existing audio file into the user's chosen format and quality.
///
/// Checkpoints are always written as AAC/MP4 because that is cheap to append to
/// and survives a crash; the final file is produced by transcoding the merged
/// result into whatever format was requested. Doing it at the end keeps the
/// crash-recovery path unchanged while still honouring the setting.
///
/// Returns an error (rather than silently keeping the intermediate file) so the
/// caller can fall back explicitly and tell the user what happened.
pub fn transcode_audio(
    input: &std::path::Path,
    output: &std::path::Path,
    settings: super::formats::AudioEncodingSettings,
) -> anyhow::Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!(
            "Cannot transcode: input file does not exist: {}",
            input.display()
        ));
    }

    let ffmpeg_path = find_ffmpeg_path().ok_or_else(|| {
        anyhow::anyhow!("FFmpeg not found. Please install FFmpeg to save recordings.")
    })?;

    let codec_args = settings.ffmpeg_codec_args();
    let metadata_args = settings.ffmpeg_metadata_args();

    let mut command = Command::new(&ffmpeg_path);
    command
        .arg("-hide_banner")
        .arg("-y") // The merged intermediate is disposable; overwrite freely.
        .arg("-i")
        .arg(input)
        .args(&codec_args)
        // Tags go after the codec selection but before the output file.
        .args(&metadata_args)
        .arg("-f")
        .arg(settings.format.ffmpeg_muxer())
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    debug!("Transcode command: {:?}", command);

    let output_result = command
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run FFmpeg for transcode: {}", e))?;

    if !output_result.status.success() {
        let stderr = String::from_utf8_lossy(&output_result.stderr);
        error!("FFmpeg transcode failed: {}", stderr);
        return Err(anyhow::anyhow!(
            "Failed to encode {}: {}",
            settings.format.label(),
            stderr.trim()
        ));
    }

    if !output.exists() {
        return Err(anyhow::anyhow!(
            "FFmpeg reported success but {} was not created",
            output.display()
        ));
    }

    tracing::info!(
        "Transcoded {} -> {} ({} @ {} kbps)",
        input.display(),
        output.display(),
        settings.format.id(),
        settings.quality.bitrate_kbps()
    );

    Ok(())
}

