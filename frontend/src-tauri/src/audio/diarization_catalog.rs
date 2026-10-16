// audio/diarization_catalog.rs
//
// Where the diarisation models come from, and whether they are usable.
//
// Two files are needed and neither is shipped with the app: the segmentation model is
// ~6 MB and the speaker-embedding model is ~38 MB, so they are downloaded on demand
// into the same per-engine model directory the other engines use, which means the
// storage-location setting and the disk-space checks apply to them too.
//
// Both are served as plain files from HuggingFace, so no archive handling is needed.
// The segmentation model ships upstream only as a `.tar.bz2`; the extracted copy on
// HuggingFace was verified byte-identical (same SHA-256) to the tarball's contents,
// which is what makes the simpler source acceptable.
//
// The base host is overridable so a mirror works, matching how the Whisper and
// Parakeet models are fetched.

use std::path::{Path, PathBuf};

/// Environment variable overriding the model download host.
pub const DIARIZATION_BASE_URL_ENV: &str = "MEETILY_DIARIZATION_BASE_URL";

/// Default host serving the diarisation models.
pub const DEFAULT_DIARIZATION_BASE_URL: &str = "https://huggingface.co";

/// One downloadable model file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiarizationArtifact {
    /// Name written to disk, which is also the name used in the repository.
    pub filename: &'static str,
    /// HuggingFace repository that serves it, as `owner/name`.
    pub repo: &'static str,
    /// Repository path, which differs from the on-disk name for the embedding model.
    pub repo_path: &'static str,
    /// Exact size in bytes, used to detect a truncated or partial download.
    pub exact_bytes: u64,
}

/// On-disk name of the segmentation model.
pub const SEGMENTATION_FILENAME: &str = "segmentation.onnx";

/// On-disk name of the speaker-embedding model.
pub const EMBEDDING_FILENAME: &str = "embedding.onnx";

/// Speech segmentation: marks speech regions and how many local speakers are active.
pub const SEGMENTATION: DiarizationArtifact = DiarizationArtifact {
    filename: SEGMENTATION_FILENAME,
    repo: "csukuangfj/sherpa-onnx-pyannote-segmentation-3-0",
    repo_path: "model.onnx",
    exact_bytes: 5_992_913,
};

/// Speaker embedding: turns a span of speech into a voice vector.
///
/// The `zh-cn` 3D-Speaker `eres2net` model, which is the Chinese-specific one and the
/// model [`super::speaker_cluster::CALIBRATED_THRESHOLD`] was measured against.
pub const EMBEDDING: DiarizationArtifact = DiarizationArtifact {
    filename: EMBEDDING_FILENAME,
    repo: "csukuangfj/speaker-embedding-models",
    repo_path: "3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx",
    exact_bytes: 39_593_761,
};

/// Everything diarisation needs.
pub const REQUIRED: &[DiarizationArtifact] = &[SEGMENTATION, EMBEDDING];

/// Optional reference clip, used only by the ignored end-to-end test.
///
/// Four Chinese speakers, from the same repository as the embedding model, so the test
/// can fetch its own fixture rather than depending on a file someone placed by hand.
pub const REFERENCE_AUDIO: DiarizationArtifact = DiarizationArtifact {
    filename: "four-speakers-zh.wav",
    repo: "csukuangfj/speaker-embedding-models",
    repo_path: "0-four-speakers-zh.wav",
    exact_bytes: 1_819_564,
};

/// Base host to download from, honouring the environment override.
pub fn base_url() -> String {
    std::env::var(DIARIZATION_BASE_URL_ENV)
        .ok()
        .and_then(|value| crate::config::normalize_base_url(&value))
        .unwrap_or_else(|| DEFAULT_DIARIZATION_BASE_URL.to_string())
}

/// Download URL for one artifact.
///
/// Takes the host as an argument rather than reading the environment, so the
/// assembly is testable without mutating process state.
pub fn download_url_for(host: &str, artifact: &DiarizationArtifact) -> String {
    format!(
        "{}/{}/resolve/main/{}",
        host.trim_end_matches('/'),
        artifact.repo,
        artifact.repo_path
    )
}

/// Download URL for one artifact, using the configured host.
pub fn download_url(artifact: &DiarizationArtifact) -> String {
    download_url_for(&base_url(), artifact)
}

/// Directory the diarisation models live in.
pub fn models_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("models").join("diarization")
}

/// Path of one artifact within `dir`.
pub fn artifact_path(dir: &Path, artifact: &DiarizationArtifact) -> PathBuf {
    dir.join(artifact.filename)
}

/// Path of the segmentation model.
pub fn segmentation_path(dir: &Path) -> PathBuf {
    artifact_path(dir, &SEGMENTATION)
}

/// Path of the embedding model.
pub fn embedding_path(dir: &Path) -> PathBuf {
    artifact_path(dir, &EMBEDDING)
}

/// Total bytes a full download needs, for the disk-space check.
pub fn total_bytes() -> u64 {
    REQUIRED.iter().map(|artifact| artifact.exact_bytes).sum()
}

/// Whether a file is present at exactly the expected size.
///
/// Size rather than existence: an interrupted download leaves a file that exists and
/// fails only when ONNX tries to parse it, which surfaces much later and as a confusing
/// error. Comparing the size catches it at the point of use.
pub fn artifact_is_complete(dir: &Path, artifact: &DiarizationArtifact) -> bool {
    std::fs::metadata(artifact_path(dir, artifact))
        .map(|metadata| metadata.is_file() && metadata.len() == artifact.exact_bytes)
        .unwrap_or(false)
}

/// Artifacts that are missing or the wrong size.
pub fn missing_artifacts(dir: &Path) -> Vec<&'static DiarizationArtifact> {
    REQUIRED
        .iter()
        .filter(|artifact| !artifact_is_complete(dir, artifact))
        .collect()
}

/// Whether diarisation can run with what is on disk.
pub fn is_ready(dir: &Path) -> bool {
    missing_artifacts(dir).is_empty()
}

/// Human-readable description of what is missing, for a log or the UI.
pub fn missing_summary(dir: &Path) -> String {
    let missing = missing_artifacts(dir);

    if missing.is_empty() {
        return "Diarisation models are ready".to_string();
    }

    let names: Vec<String> = missing
        .iter()
        .map(|artifact| {
            let path = artifact_path(dir, artifact);
            match std::fs::metadata(&path) {
                // Distinguish "absent" from "present but the wrong size", because the
                // fix differs: download versus re-download.
                Ok(metadata) => format!(
                    "{} (incomplete: {} of {} bytes)",
                    artifact.filename,
                    metadata.len(),
                    artifact.exact_bytes
                ),
                Err(_) => format!("{} (missing)", artifact.filename),
            }
        })
        .collect();

    format!(
        "Speaker separation needs {} more file(s) ({} MB): {}",
        missing.len(),
        missing
            .iter()
            .map(|artifact| artifact.exact_bytes)
            .sum::<u64>()
            / 1_048_576,
        names.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    /// Write a file of the exact expected size so completeness checks can pass.
    fn write_sized(path: &Path, size: u64) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create dir");
        }
        std::fs::write(path, vec![0u8; size as usize]).expect("write");
    }

    #[test]
    fn the_url_points_at_the_repository_file() {
        let url = download_url_for("https://huggingface.co", &SEGMENTATION);
        assert_eq!(
            url,
            "https://huggingface.co/csukuangfj/sherpa-onnx-pyannote-segmentation-3-0/resolve/main/model.onnx"
        );
    }

    #[test]
    fn the_embedding_url_uses_the_repository_path_not_the_local_name() {
        // The file is stored under its long upstream name but written to disk as
        // `embedding.onnx`; using the local name in the URL would 404.
        let url = download_url_for("https://huggingface.co", &EMBEDDING);
        assert!(url.contains("3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx"));
        assert!(!url.ends_with("embedding.onnx"));
    }

    #[test]
    fn a_trailing_slash_on_the_host_does_not_double_up() {
        let url = download_url_for("https://hf-mirror.com/", &SEGMENTATION);
        assert!(url.starts_with("https://hf-mirror.com/csukuangfj/"), "got: {url}");
        assert!(!url.contains("//csukuangfj"), "got: {url}");
    }

    #[test]
    fn the_models_directory_matches_the_other_engines() {
        let dir = models_dir(Path::new("C:/appdata"));
        assert!(dir.ends_with("models/diarization") || dir.ends_with("models\\diarization"));
    }

    #[test]
    fn artifact_paths_are_flat_and_named_clearly() {
        let dir = models_dir(Path::new("C:/appdata"));
        assert!(segmentation_path(&dir).ends_with(SEGMENTATION_FILENAME));
        assert!(embedding_path(&dir).ends_with(EMBEDDING_FILENAME));
    }

    #[test]
    fn nothing_is_ready_in_an_empty_directory() {
        let dir = temp_dir();
        assert!(!is_ready(dir.path()));
        assert_eq!(missing_artifacts(dir.path()).len(), REQUIRED.len());
    }

    #[test]
    fn a_correctly_sized_file_counts_as_complete() {
        let dir = temp_dir();
        write_sized(&segmentation_path(dir.path()), SEGMENTATION.exact_bytes);

        assert!(artifact_is_complete(dir.path(), &SEGMENTATION));
        assert!(!artifact_is_complete(dir.path(), &EMBEDDING));
    }

    #[test]
    fn a_truncated_file_does_not_count_as_complete() {
        // The important case: an interrupted download leaves a file that exists but
        // cannot be parsed, which would otherwise fail much later with a confusing error.
        let dir = temp_dir();
        write_sized(&segmentation_path(dir.path()), SEGMENTATION.exact_bytes / 2);

        assert!(!artifact_is_complete(dir.path(), &SEGMENTATION));
        let missing = missing_artifacts(dir.path());
        assert!(missing.contains(&&SEGMENTATION));
    }

    #[test]
    fn an_oversized_file_does_not_count_as_complete() {
        let dir = temp_dir();
        write_sized(&segmentation_path(dir.path()), SEGMENTATION.exact_bytes + 1);
        assert!(!artifact_is_complete(dir.path(), &SEGMENTATION));
    }

    #[test]
    fn a_directory_where_a_file_is_expected_is_not_complete() {
        let dir = temp_dir();
        std::fs::create_dir_all(segmentation_path(dir.path())).expect("mkdir");
        assert!(!artifact_is_complete(dir.path(), &SEGMENTATION));
    }

    #[test]
    fn everything_present_and_sized_is_ready() {
        let dir = temp_dir();
        for artifact in REQUIRED {
            write_sized(&artifact_path(dir.path(), artifact), artifact.exact_bytes);
        }

        assert!(is_ready(dir.path()));
        assert!(missing_artifacts(dir.path()).is_empty());
    }

    #[test]
    fn the_total_is_the_sum_of_both_models() {
        assert_eq!(total_bytes(), SEGMENTATION.exact_bytes + EMBEDDING.exact_bytes);
        // Roughly 45 MB, which is why these are downloaded rather than bundled.
        assert_eq!(total_bytes(), 45_586_674);
    }

    #[test]
    fn the_summary_says_ready_when_ready() {
        let dir = temp_dir();
        for artifact in REQUIRED {
            write_sized(&artifact_path(dir.path(), artifact), artifact.exact_bytes);
        }
        assert!(missing_summary(dir.path()).contains("ready"));
    }

    #[test]
    fn the_summary_distinguishes_missing_from_incomplete() {
        // The two need different fixes, so the message must not blur them.
        let dir = temp_dir();
        write_sized(&segmentation_path(dir.path()), 100);

        let summary = missing_summary(dir.path());
        assert!(summary.contains("incomplete"), "got: {summary}");
        assert!(summary.contains("missing"), "got: {summary}");
    }

    #[test]
    fn the_summary_reports_the_bytes_still_needed() {
        let dir = temp_dir();
        let summary = missing_summary(dir.path());

        let expected_mb = (SEGMENTATION.exact_bytes + EMBEDDING.exact_bytes) / 1_048_576;
        assert!(
            summary.contains(&expected_mb.to_string()),
            "expected {expected_mb} MB in: {summary}"
        );
    }

    #[test]
    fn the_reference_clip_comes_from_the_embedding_repository() {
        // So the ignored end-to-end test can fetch its own fixture instead of relying
        // on a file someone placed by hand.
        assert_eq!(REFERENCE_AUDIO.repo, EMBEDDING.repo);
        assert!(REFERENCE_AUDIO.repo_path.ends_with(".wav"));
    }
}
