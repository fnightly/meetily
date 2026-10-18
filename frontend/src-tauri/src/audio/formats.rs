// audio/formats.rs
//
// Recording output formats and quality settings.
//
// Before this, `RecordingPreferences.file_format` was saved to disk, shown in the
// settings UI, and **never read by anything**: `encode_single_audio` hardcoded
// AAC 192 kbit/s inside an MP4 container. The UI therefore promised a format the
// app did not produce. This module is the single source of truth for what a
// recording can be written as.

use serde::{Deserialize, Serialize};

/// Container/codec pair a recording can be written as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordingFormat {
    /// MPEG-4 audio (AAC). The historical default: good compression, universally playable.
    M4a,
    /// MPEG-1 Layer III. The most widely requested portable format.
    Mp3,
    /// Uncompressed PCM. Largest files, zero quality loss, useful for post-processing.
    Wav,
    /// Lossless compression.
    Flac,
    /// Opus in an Ogg container. Best quality per byte for speech.
    Ogg,
}

impl RecordingFormat {
    /// Every supported format, in the order the UI should list them.
    pub const ALL: &'static [RecordingFormat] = &[
        RecordingFormat::M4a,
        RecordingFormat::Mp3,
        RecordingFormat::Wav,
        RecordingFormat::Flac,
        RecordingFormat::Ogg,
    ];

    /// Stable identifier stored in preferences.
    pub fn id(self) -> &'static str {
        match self {
            RecordingFormat::M4a => "m4a",
            RecordingFormat::Mp3 => "mp3",
            RecordingFormat::Wav => "wav",
            RecordingFormat::Flac => "flac",
            RecordingFormat::Ogg => "ogg",
        }
    }

    /// File extension used for the final recording.
    pub fn extension(self) -> &'static str {
        self.id()
    }

    /// Human-readable name for the settings UI.
    pub fn label(self) -> &'static str {
        match self {
            RecordingFormat::M4a => "M4A (AAC)",
            RecordingFormat::Mp3 => "MP3",
            RecordingFormat::Wav => "WAV (uncompressed)",
            RecordingFormat::Flac => "FLAC (lossless)",
            RecordingFormat::Ogg => "OGG (Opus)",
        }
    }

    /// FFmpeg audio encoder for this format.
    pub fn ffmpeg_encoder(self) -> &'static str {
        match self {
            RecordingFormat::M4a => "aac",
            RecordingFormat::Mp3 => "libmp3lame",
            RecordingFormat::Wav => "pcm_s16le",
            RecordingFormat::Flac => "flac",
            RecordingFormat::Ogg => "libopus",
        }
    }

    /// FFmpeg container/muxer name (`-f`).
    pub fn ffmpeg_muxer(self) -> &'static str {
        match self {
            RecordingFormat::M4a => "mp4",
            RecordingFormat::Mp3 => "mp3",
            RecordingFormat::Wav => "wav",
            RecordingFormat::Flac => "flac",
            RecordingFormat::Ogg => "ogg",
        }
    }

    /// Whether the format can represent every sample it is given.
    ///
    /// Lossless formats ignore a bitrate setting entirely, so the UI should hide
    /// the quality control for them rather than offering a no-op.
    pub fn is_lossless(self) -> bool {
        matches!(self, RecordingFormat::Wav | RecordingFormat::Flac)
    }

    /// Parse a stored identifier, tolerating legacy and alias spellings.
    ///
    /// Older installs stored `"mp4"` for what is really M4A audio, and users may
    /// have `"aac"`. Both map to [`RecordingFormat::M4a`] so an existing config
    /// keeps working and the produced file matches what was already on disk.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "m4a" | "mp4" | "aac" => Some(RecordingFormat::M4a),
            "mp3" => Some(RecordingFormat::Mp3),
            "wav" => Some(RecordingFormat::Wav),
            "flac" => Some(RecordingFormat::Flac),
            "ogg" | "opus" => Some(RecordingFormat::Ogg),
            _ => None,
        }
    }

    /// Parse, falling back to the default rather than failing.
    pub fn parse_or_default(raw: &str) -> Self {
        Self::parse(raw).unwrap_or_default()
    }
}

impl Default for RecordingFormat {
    /// M4A preserves the behaviour of existing installs.
    fn default() -> Self {
        RecordingFormat::M4a
    }
}

/// Encoding quality, expressed as an audio bitrate for lossy formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordingQuality {
    /// ~64 kbit/s. Small files; acceptable for speech-only notes.
    Low,
    /// ~128 kbit/s. The default: transparent enough for speech.
    Medium,
    /// ~192 kbit/s. Better for music or mixed content.
    High,
    /// ~256 kbit/s. Largest lossy option.
    Best,
}

impl RecordingQuality {
    pub const ALL: &'static [RecordingQuality] = &[
        RecordingQuality::Low,
        RecordingQuality::Medium,
        RecordingQuality::High,
        RecordingQuality::Best,
    ];

    /// Stable identifier stored in preferences.
    pub fn id(self) -> &'static str {
        match self {
            RecordingQuality::Low => "low",
            RecordingQuality::Medium => "medium",
            RecordingQuality::High => "high",
            RecordingQuality::Best => "best",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            RecordingQuality::Low => "Low (64 kbps)",
            RecordingQuality::Medium => "Medium (128 kbps)",
            RecordingQuality::High => "High (192 kbps)",
            RecordingQuality::Best => "Best (256 kbps)",
        }
    }

    /// Bitrate in kbit/s.
    pub fn bitrate_kbps(self) -> u32 {
        match self {
            RecordingQuality::Low => 64,
            RecordingQuality::Medium => 128,
            RecordingQuality::High => 192,
            RecordingQuality::Best => 256,
        }
    }

    /// Parse a stored identifier.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "low" => Some(RecordingQuality::Low),
            "medium" | "med" | "default" => Some(RecordingQuality::Medium),
            "high" => Some(RecordingQuality::High),
            "best" => Some(RecordingQuality::Best),
            _ => None,
        }
    }

    pub fn parse_or_default(raw: &str) -> Self {
        Self::parse(raw).unwrap_or_default()
    }
}

impl Default for RecordingQuality {
    fn default() -> Self {
        RecordingQuality::Medium
    }
}

/// Descriptive tags written into the finished recording.
///
/// Borrowed from OnlyR, which stamps track title / album / genre into its MP3s:
/// a recording that carries its own meeting name and date is far easier to
/// identify once it leaves the app and lands in a file manager or music player.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecordingTags {
    pub title: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub comment: Option<String>,
}

impl RecordingTags {
    /// Tags for a meeting recording.
    ///
    /// `created_at` is an RFC 3339 timestamp; only the date part is used, because
    /// a full timestamp in the `date` field is not portable across containers.
    pub fn for_meeting(meeting_name: &str, created_at: &str) -> Self {
        let date = created_at.split('T').next().unwrap_or(created_at);

        Self {
            title: Some(meeting_name.to_string()),
            album: Some("会见".to_string()),
            genre: Some("Speech".to_string()),
            comment: Some(format!("Recorded by 会见 on {}", date)),
        }
    }

    /// Filter out empty values so FFmpeg never receives `-metadata title=`.
    fn entries(&self) -> Vec<(&'static str, &str)> {
        let mut entries = Vec::new();

        for (key, value) in [
            ("title", self.title.as_deref()),
            ("album", self.album.as_deref()),
            ("genre", self.genre.as_deref()),
            ("comment", self.comment.as_deref()),
        ] {
            if let Some(value) = value {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    entries.push((key, trimmed));
                }
            }
        }

        entries
    }
}

/// Fully resolved encoding settings for one recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioEncodingSettings {
    pub format: RecordingFormat,
    pub quality: RecordingQuality,
    /// Tags to embed. `None` skips metadata entirely.
    pub tags: Option<RecordingTags>,
    /// Copy the audio stream instead of re-encoding it.
    ///
    /// Set when the source is already in the target codec — the merged AAC/MP4
    /// intermediate being written out as M4A — so the finalize pass only remuxes
    /// and stamps tags rather than decoding and re-encoding the whole recording.
    pub copy_stream: bool,
}

impl Default for AudioEncodingSettings {
    fn default() -> Self {
        Self {
            format: RecordingFormat::default(),
            quality: RecordingQuality::default(),
            tags: None,
            copy_stream: false,
        }
    }
}

impl AudioEncodingSettings {
    pub fn new(format: RecordingFormat, quality: RecordingQuality) -> Self {
        Self {
            format,
            quality,
            tags: None,
            copy_stream: false,
        }
    }

    /// Attach tags, builder-style.
    pub fn with_tags(mut self, tags: RecordingTags) -> Self {
        self.tags = Some(tags);
        self
    }

    /// Build from stored strings, falling back to defaults for unknown values.
    pub fn from_stored(format: &str, quality: &str) -> Self {
        Self {
            format: RecordingFormat::parse_or_default(format),
            quality: RecordingQuality::parse_or_default(quality),
            tags: None,
            copy_stream: false,
        }
    }

    /// File name for the finished recording inside the meeting folder.
    pub fn output_file_name(&self) -> String {
        format!("audio.{}", self.format.extension())
    }

    /// FFmpeg arguments that write the tags, or an empty list when there are none.
    pub fn ffmpeg_metadata_args(&self) -> Vec<String> {
        let Some(tags) = &self.tags else {
            return Vec::new();
        };

        let mut args = Vec::new();
        for (key, value) in tags.entries() {
            args.push("-metadata".to_string());
            args.push(format!("{key}={value}"));
        }

        args
    }

    /// Target bitrate for a lossy encoding of `channels` channels.
    ///
    /// Bitrate is per audio stream, so stereo needs roughly twice the mono
    /// budget to sound the same. The 320 kbit/s ceiling keeps the value inside
    /// what AAC and MP3 encoders accept and matches the UI's quality labels;
    /// mono is unchanged (64/128/192/256).
    pub fn lossy_bitrate_kbps(&self, channels: u16) -> u32 {
        (self.quality.bitrate_kbps() * channels.max(1) as u32).min(320)
    }

    /// FFmpeg arguments that select the codec and quality for this format.
    ///
    /// `channels` is the session's channel count: it scales the lossy bitrate
    /// and is written explicitly for PCM, where omitting it would let FFmpeg
    /// keep the input layout and where the old hardcoded `-ac 1` folded stereo
    /// back to mono.
    ///
    /// Lossless formats deliberately omit `-b:a`: passing a bitrate to FLAC or
    /// PCM is either rejected or silently ignored, and omitting it keeps the
    /// command honest about what is happening.
    ///
    /// When [`Self::copy_stream`] is set, the audio is copied untouched and only
    /// the container and tags are rewritten.
    pub fn ffmpeg_codec_args(&self, channels: u16) -> Vec<String> {
        if self.copy_stream {
            return vec!["-c:a".to_string(), "copy".to_string()];
        }

        // Zero would ask for a zero-channel output; the recording contract only
        // ever sends 1 or 2, and mono is the safe reading of a missing value.
        let channels = channels.max(1);

        let mut args = vec!["-c:a".to_string(), self.format.ffmpeg_encoder().to_string()];

        if !self.format.is_lossless() {
            args.push("-b:a".to_string());
            args.push(format!("{}k", self.lossy_bitrate_kbps(channels)));
        }

        match self.format {
            // AAC-LC is the profile every player understands; `+faststart` makes
            // the file streamable.
            RecordingFormat::M4a => {
                args.push("-profile:a".to_string());
                args.push("aac_low".to_string());
                args.push("-movflags".to_string());
                args.push("+faststart".to_string());
            }
            // MP3 at a constant bitrate is the most compatible configuration.
            RecordingFormat::Mp3 => {
                args.push("-write_xing".to_string());
                args.push("1".to_string());
            }
            // Opus is inherently variable-bitrate; constrain it to the requested
            // target instead of passing `-b:a` alone.
            RecordingFormat::Ogg => {
                args.push("-vbr".to_string());
                args.push("on".to_string());
            }
            RecordingFormat::Wav => {
                // Without an explicit count FFmpeg keeps the input layout; a
                // hardcoded mono here silently folded stereo recordings down.
                args.push("-ac".to_string());
                args.push(channels.to_string());
            }
            RecordingFormat::Flac => {
                args.push("-compression_level".to_string());
                args.push("5".to_string());
            }
        }

        args
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_format_round_trips_through_its_id() {
        for format in RecordingFormat::ALL {
            assert_eq!(
                RecordingFormat::parse(format.id()),
                Some(*format),
                "id {} did not round-trip",
                format.id()
            );
        }
    }

    #[test]
    fn legacy_mp4_and_aac_ids_map_to_m4a() {
        // Existing installs stored "mp4" for what is really AAC audio; changing
        // that silently would move the file name out from under old recordings.
        assert_eq!(RecordingFormat::parse("mp4"), Some(RecordingFormat::M4a));
        assert_eq!(RecordingFormat::parse("aac"), Some(RecordingFormat::M4a));
        assert_eq!(RecordingFormat::parse("MP4"), Some(RecordingFormat::M4a));
    }

    #[test]
    fn unknown_format_falls_back_to_m4a() {
        assert_eq!(RecordingFormat::parse("nope"), None);
        assert_eq!(RecordingFormat::parse_or_default("nope"), RecordingFormat::M4a);
        assert_eq!(RecordingFormat::parse_or_default(""), RecordingFormat::M4a);
    }

    #[test]
    fn extension_matches_id_for_every_format() {
        for format in RecordingFormat::ALL {
            assert_eq!(format.extension(), format.id());
        }
    }

    #[test]
    fn lossless_formats_omit_the_bitrate_flag() {
        for format in [RecordingFormat::Wav, RecordingFormat::Flac] {
            assert!(format.is_lossless());
            let args = AudioEncodingSettings::new(format, RecordingQuality::Best).ffmpeg_codec_args(1);
            assert!(
                !args.iter().any(|a| a == "-b:a"),
                "{} must not pass a bitrate",
                format.id()
            );
        }
    }

    #[test]
    fn lossy_formats_pass_a_bitrate() {
        for format in [RecordingFormat::M4a, RecordingFormat::Mp3, RecordingFormat::Ogg] {
            assert!(!format.is_lossless());
            let args = AudioEncodingSettings::new(format, RecordingQuality::High).ffmpeg_codec_args(1);
            let position = args.iter().position(|a| a == "-b:a");
            assert!(position.is_some(), "{} must pass a bitrate", format.id());
            assert_eq!(args[position.unwrap() + 1], "192k");
        }
    }

    #[test]
    fn codec_args_start_with_the_expected_encoder() {
        assert_eq!(
            AudioEncodingSettings::new(RecordingFormat::Mp3, RecordingQuality::Medium)
                .ffmpeg_codec_args(1)[..2],
            ["-c:a".to_string(), "libmp3lame".to_string()]
        );
        assert_eq!(
            AudioEncodingSettings::new(RecordingFormat::Ogg, RecordingQuality::Medium)
                .ffmpeg_codec_args(1)[..2],
            ["-c:a".to_string(), "libopus".to_string()]
        );
        assert_eq!(
            AudioEncodingSettings::new(RecordingFormat::M4a, RecordingQuality::Medium)
                .ffmpeg_codec_args(1)[..2],
            ["-c:a".to_string(), "aac".to_string()]
        );
    }

    #[test]
    fn m4a_keeps_streaming_optimisation() {
        let args = AudioEncodingSettings::new(RecordingFormat::M4a, RecordingQuality::Medium)
            .ffmpeg_codec_args(1);
        assert!(args.iter().any(|a| a == "+faststart"));
        assert!(args.iter().any(|a| a == "aac_low"));
    }

    #[test]
    fn output_file_name_uses_the_format_extension() {
        assert_eq!(
            AudioEncodingSettings::new(RecordingFormat::Mp3, RecordingQuality::Medium)
                .output_file_name(),
            "audio.mp3"
        );
        assert_eq!(
            AudioEncodingSettings::new(RecordingFormat::Flac, RecordingQuality::Low)
                .output_file_name(),
            "audio.flac"
        );
    }

    #[test]
    fn quality_ids_round_trip_and_cover_every_variant() {
        for quality in RecordingQuality::ALL {
            assert_eq!(RecordingQuality::parse(quality.id()), Some(*quality));
        }
    }

    #[test]
    fn quality_bitrates_increase_with_quality() {
        let rates: Vec<u32> = RecordingQuality::ALL
            .iter()
            .map(|q| q.bitrate_kbps())
            .collect();
        assert!(
            rates.windows(2).all(|w| w[0] < w[1]),
            "bitrates must strictly increase: {rates:?}"
        );
    }

    #[test]
    fn unknown_quality_falls_back_to_medium() {
        assert_eq!(RecordingQuality::parse_or_default("ultra"), RecordingQuality::Medium);
        assert_eq!(RecordingQuality::parse_or_default(""), RecordingQuality::Medium);
    }

    #[test]
    fn from_stored_survives_broken_preferences() {
        // A corrupt or older preferences file must not break recording.
        let settings = AudioEncodingSettings::from_stored("", "");
        assert_eq!(settings.format, RecordingFormat::M4a);
        assert_eq!(settings.quality, RecordingQuality::Medium);
    }

    #[test]
    fn every_format_has_distinct_encoder_and_extension() {
        for (i, a) in RecordingFormat::ALL.iter().enumerate() {
            for b in &RecordingFormat::ALL[i + 1..] {
                assert_ne!(a.extension(), b.extension(), "duplicate extension");
            }
        }
    }

    #[test]
    fn format_labels_are_non_empty() {
        for format in RecordingFormat::ALL {
            assert!(!format.label().is_empty());
        }
        for quality in RecordingQuality::ALL {
            assert!(!quality.label().is_empty());
        }
    }

    #[test]
    fn meeting_tags_carry_the_name_and_date() {
        let tags = RecordingTags::for_meeting("Team Standup", "2026-10-14T09:30:00+08:00");

        assert_eq!(tags.title.as_deref(), Some("Team Standup"));
        assert_eq!(tags.album.as_deref(), Some("会见"));
        assert_eq!(tags.genre.as_deref(), Some("Speech"));
        // Only the date part: a full timestamp is not portable across containers.
        assert_eq!(
            tags.comment.as_deref(),
            Some("Recorded by 会见 on 2026-10-14")
        );
    }

    #[test]
    fn meeting_tags_survive_a_timestamp_without_a_time_part() {
        let tags = RecordingTags::for_meeting("Standup", "2026-10-14");
        assert!(tags.comment.unwrap().contains("2026-10-14"));
    }

    #[test]
    fn metadata_args_are_empty_without_tags() {
        let settings = AudioEncodingSettings::new(RecordingFormat::Mp3, RecordingQuality::Medium);
        assert!(settings.ffmpeg_metadata_args().is_empty());
    }

    #[test]
    fn metadata_args_pair_each_key_with_its_value() {
        let settings = AudioEncodingSettings::new(RecordingFormat::Mp3, RecordingQuality::Medium)
            .with_tags(RecordingTags::for_meeting("Standup", "2026-10-14T10:00:00Z"));

        let args = settings.ffmpeg_metadata_args();
        // Every value must be preceded by its own `-metadata` flag.
        assert_eq!(args.len() % 2, 0);
        for pair in args.chunks(2) {
            assert_eq!(pair[0], "-metadata");
            assert!(pair[1].contains('='), "malformed metadata arg: {}", pair[1]);
        }
        assert!(args.iter().any(|a| a == "title=Standup"));
    }

    #[test]
    fn metadata_args_skip_blank_values() {
        // FFmpeg would otherwise receive `-metadata title=`, which writes an empty
        // tag rather than omitting it.
        let settings = AudioEncodingSettings::new(RecordingFormat::Mp3, RecordingQuality::Medium)
            .with_tags(RecordingTags {
                title: Some("  ".to_string()),
                album: None,
                genre: Some("Speech".to_string()),
                comment: Some(String::new()),
            });

        let args = settings.ffmpeg_metadata_args();
        assert_eq!(args, vec!["-metadata".to_string(), "genre=Speech".to_string()]);
    }

    #[test]
    fn copy_stream_replaces_the_encoder_with_a_stream_copy() {
        let mut settings =
            AudioEncodingSettings::new(RecordingFormat::M4a, RecordingQuality::Best);
        assert_eq!(
            settings.ffmpeg_codec_args(1)[..2],
            ["-c:a".to_string(), "aac".to_string()]
        );

        settings.copy_stream = true;
        // Remuxing must not re-encode: otherwise finalizing a long recording
        // would cost a full decode/encode pass for a container-only change.
        assert_eq!(
            settings.ffmpeg_codec_args(1),
            vec!["-c:a".to_string(), "copy".to_string()]
        );
    }

    #[test]
    fn with_tags_is_chainable_and_replaces_nothing_else() {
        let settings = AudioEncodingSettings::new(RecordingFormat::Flac, RecordingQuality::Low)
            .with_tags(RecordingTags::for_meeting("X", "2026-01-01"));

        assert_eq!(settings.format, RecordingFormat::Flac);
        assert_eq!(settings.quality, RecordingQuality::Low);
        assert!(settings.tags.is_some());
        assert!(!settings.copy_stream);
    }

    fn bitrate_arg(args: &[String]) -> Option<&str> {
        let index = args.iter().position(|a| a == "-b:a")?;
        args.get(index + 1).map(|value| value.as_str())
    }

    #[test]
    fn stereo_scales_the_lossy_bitrate() {
        // Bitrate is per audio stream, so stereo needs roughly twice the mono
        // budget to sound the same.
        let mono = AudioEncodingSettings::new(RecordingFormat::M4a, RecordingQuality::Medium)
            .ffmpeg_codec_args(1);
        assert_eq!(bitrate_arg(&mono), Some("128k"));

        let stereo = AudioEncodingSettings::new(RecordingFormat::M4a, RecordingQuality::Medium)
            .ffmpeg_codec_args(2);
        assert_eq!(bitrate_arg(&stereo), Some("256k"));

        let low_stereo = AudioEncodingSettings::new(RecordingFormat::Mp3, RecordingQuality::Low)
            .ffmpeg_codec_args(2);
        assert_eq!(bitrate_arg(&low_stereo), Some("128k"));
    }

    #[test]
    fn stereo_bitrate_is_capped_at_320() {
        // High (192) and Best (256) double past what AAC/MP3 encoders accept.
        for quality in [RecordingQuality::High, RecordingQuality::Best] {
            let args = AudioEncodingSettings::new(RecordingFormat::M4a, quality)
                .ffmpeg_codec_args(2);
            assert_eq!(bitrate_arg(&args), Some("320k"), "quality {quality:?}");
        }
    }

    #[test]
    fn wav_writes_the_requested_channel_count() {
        let mono = AudioEncodingSettings::new(RecordingFormat::Wav, RecordingQuality::Best)
            .ffmpeg_codec_args(1);
        let stereo = AudioEncodingSettings::new(RecordingFormat::Wav, RecordingQuality::Best)
            .ffmpeg_codec_args(2);

        // `-ac` is what stops a stereo recording from being folded to mono.
        fn channel_arg(args: &[String]) -> String {
            let index = args
                .iter()
                .position(|a| a == "-ac")
                .expect("WAV must pass -ac");
            args[index + 1].clone()
        }

        assert_eq!(channel_arg(&mono), "1");
        assert_eq!(channel_arg(&stereo), "2");
        // PCM has no bitrate, stereo or not.
        assert!(!stereo.iter().any(|a| a == "-b:a"));
    }

    #[test]
    fn zero_channels_read_as_mono() {
        // Defensive: a missing channel count must not request zero channels.
        let args = AudioEncodingSettings::new(RecordingFormat::Wav, RecordingQuality::Medium)
            .ffmpeg_codec_args(0);
        let index = args
            .iter()
            .position(|a| a == "-ac")
            .expect("WAV must pass -ac");
        assert_eq!(args[index + 1], "1");

        let lossy = AudioEncodingSettings::new(RecordingFormat::M4a, RecordingQuality::Medium)
            .ffmpeg_codec_args(0);
        assert_eq!(bitrate_arg(&lossy), Some("128k"));
    }

    #[test]
    fn lossless_formats_do_not_gain_channel_flags() {
        // FLAC keeps the input layout; only WAV spells the channel count out.
        let flac = AudioEncodingSettings::new(RecordingFormat::Flac, RecordingQuality::Best)
            .ffmpeg_codec_args(2);
        assert!(!flac.iter().any(|a| a == "-b:a"));
        assert!(!flac.iter().any(|a| a == "-ac"));
    }

    #[test]
    fn copy_stream_stays_a_plain_stream_copy_for_stereo() {
        let mut settings =
            AudioEncodingSettings::new(RecordingFormat::M4a, RecordingQuality::Best);
        settings.copy_stream = true;

        // Remuxing must not gain channel or bitrate flags: it re-encodes nothing.
        assert_eq!(
            settings.ffmpeg_codec_args(2),
            vec!["-c:a".to_string(), "copy".to_string()]
        );
    }
}
