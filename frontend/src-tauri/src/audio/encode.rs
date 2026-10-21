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

/// Encode interleaved `f32le` samples straight into AAC/MP4.
///
/// `channels` is the interleaving of `data` and `bitrate_kbps` the total budget
/// for all channels: the caller passes a channel-aware value rather than a mono
/// figure so a stereo recording is not encoded at half its intended quality.
pub fn encode_single_audio(
    data: &[u8],
    sample_rate: u32,
    channels: u16,
    bitrate_kbps: u32,
    output_path: &PathBuf,
) -> anyhow::Result<()> {
    debug!(
        "Starting FFmpeg process for {} bytes of audio data ({} Hz, {} ch, {} kbps)",
        data.len(),
        sample_rate,
        channels,
        bitrate_kbps
    );

    if data.is_empty() {
        return Err(anyhow::anyhow!("No audio data provided for encoding"));
    }

    let ffmpeg_path = find_ffmpeg_path().ok_or_else(|| {
        anyhow::anyhow!("FFmpeg not found. Please install FFmpeg to save recordings.")
    })?;

    debug!("Using FFmpeg at: {:?}", ffmpeg_path);

    let bitrate_arg = format!("{}k", bitrate_kbps);

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
            bitrate_arg.as_str(),
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
///
/// `channels` is the channel count of `input` and of the output: it scales the
/// lossy bitrate and is written explicitly for PCM, so a stereo recording is
/// not silently folded to mono on the way out.
pub fn transcode_audio(
    input: &std::path::Path,
    output: &std::path::Path,
    settings: super::formats::AudioEncodingSettings,
    channels: u16,
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

    let codec_args = settings.ffmpeg_codec_args(channels);
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

    let bitrate_label = if settings.format.is_lossless() {
        "lossless".to_string()
    } else {
        format!("{} kbps", settings.lossy_bitrate_kbps(channels))
    };
    tracing::info!(
        "Transcoded {} -> {} ({} @ {}, {} ch)",
        input.display(),
        output.display(),
        settings.format.id(),
        bitrate_label,
        channels.max(1)
    );

    Ok(())
}

/// Re-encode any audio file as mono AAC in an .m4a container at the given bitrate.
///
/// Used for size-limited uploads (Notion sync), where the recording's original
/// format and bitrate can be far larger than the destination accepts. Mono keeps
/// speech intelligible at the low bitrates such limits require.
pub fn transcode_audio_to_mono_aac(
    input: &std::path::Path,
    output: &std::path::Path,
    bitrate_kbps: u32,
) -> anyhow::Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!(
            "Cannot compress audio: input file does not exist: {}",
            input.display()
        ));
    }

    let ffmpeg_path = find_ffmpeg_path().ok_or_else(|| {
        anyhow::anyhow!("FFmpeg not found. Please install FFmpeg to sync recordings.")
    })?;

    let bitrate_arg = format!("{}k", bitrate_kbps);
    debug!(
        "Compressing {} -> {} (mono AAC, {} kbps)",
        input.display(),
        output.display(),
        bitrate_kbps
    );

    let mut command = Command::new(&ffmpeg_path);
    command
        .arg("-hide_banner")
        .arg("-y")
        .arg("-i")
        .arg(input)
        .args(["-vn", "-ac", "1", "-c:a", "aac", "-b:a"])
        .arg(&bitrate_arg)
        .args([
            "-profile:a",
            "aac_low",
            "-movflags",
            "+faststart",
            "-f",
            "mp4",
        ])
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

    let result = command
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run FFmpeg to compress the audio: {}", e))?;

    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        error!("FFmpeg audio compression failed: {}", stderr);
        return Err(anyhow::anyhow!(
            "Failed to compress the recording: {}",
            stderr.trim()
        ));
    }

    if !output.exists() {
        return Err(anyhow::anyhow!(
            "FFmpeg reported success but the compressed file was not created: {}",
            output.display()
        ));
    }

    Ok(())
}

